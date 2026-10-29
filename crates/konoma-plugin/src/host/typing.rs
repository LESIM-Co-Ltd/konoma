//! The text input a canvas can ask for (`CanvasView::text_input`), and the line of keys and
//! actions that wait for a session a job holds. Pure data in, decisions out: the host keeps the
//! queues and the latch where it likes and passes them in.
//!
//! - While the canvas on screen asks for it and `Esc` has not left it ([`is_capturing`]), the host
//!   takes the typing: every key but `Esc` becomes a [`Key`] ([`key_of`]) and goes to
//!   `Session::key`; a paste is one `KeyCode::Paste`.
//! - `Esc` is the host's: it leaves at once ([`admit_key`] sets the latch) and the plugin is told
//!   with an `Esc` key -- the last place of the queue is kept for it. **Only the user takes the
//!   typing back** ([`retake`]): after `Esc` the host captures again only when the canvas that
//!   asks for it answers something the user did (a declared action they pressed, or opening the
//!   file again). A timer tick, a refit, the answer to a key never do.
//! - Keys are never dropped and never reordered: while a job holds the session they wait in a
//!   bounded queue ([`MAX_QUEUED_KEYS`]; a paste is cut at [`MAX_PASTE_BYTES`], the pastes
//!   together at [`MAX_QUEUED_PASTE_BYTES`]) and go, in order, with the declared actions that were
//!   pressed meanwhile ([`next_job`]; at most [`MAX_QUEUED_ACTS`] of those). Past a bound the new
//!   input is refused with a [`Notice`].
//! - `Outcome::Unhandled` is nothing: no code here, because nothing is done with it.

use crossterm::event::{KeyCode as C, KeyEvent, KeyModifiers as M};

use crate::key::{Key, KeyCode, KeyMods};

/// How many keys wait for a session that a job holds, at most.
pub const MAX_QUEUED_KEYS: usize = 4096;

/// The longest paste handed to a plugin, in bytes; a longer one is cut (at a character boundary).
pub const MAX_PASTE_BYTES: usize = 1 << 20;

/// The most bytes of pasted text that wait in the queue together (the sum over the pastes).
pub const MAX_QUEUED_PASTE_BYTES: usize = 4 << 20;

/// How many declared-action presses wait for the session at most; past it the press is refused
/// with a notice (never reordered, never dropped silently).
pub const MAX_QUEUED_ACTS: usize = 256;

/// What the host should tell the user, in its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// "the plugin is not answering: input refused".
    InputFull,
    /// "the paste was too long: cut for the plugin".
    PasteCut,
}

/// The result of offering input to the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admitted {
    /// Whether it was queued (the host then tries to send: [`next_job`]).
    pub queued: bool,
    pub notice: Option<Notice>,
}

impl Admitted {
    const fn queued() -> Admitted {
        Admitted {
            queued: true,
            notice: None,
        }
    }
    const fn refused() -> Admitted {
        Admitted {
            queued: false,
            notice: Some(Notice::InputFull),
        }
    }
}

/// A terminal key as the plugin receives it. `None` for the keys a plugin has no name for (the
/// media keys, `Insert`, the lock keys); those are ignored while the plugin takes the typing.
pub fn key_of(key: &KeyEvent) -> Option<Key> {
    let code = match key.code {
        C::Char(c) => KeyCode::Char(c),
        C::Enter => KeyCode::Enter,
        C::Backspace => KeyCode::Backspace,
        C::Delete => KeyCode::Delete,
        C::Tab => KeyCode::Tab,
        C::BackTab => KeyCode::BackTab,
        C::Left => KeyCode::Left,
        C::Right => KeyCode::Right,
        C::Up => KeyCode::Up,
        C::Down => KeyCode::Down,
        C::Home => KeyCode::Home,
        C::End => KeyCode::End,
        C::PageUp => KeyCode::PageUp,
        C::PageDown => KeyCode::PageDown,
        C::Esc => KeyCode::Esc,
        C::F(n) => KeyCode::F(n),
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
    if key.modifiers.contains(M::SHIFT) && !matches!(code, KeyCode::Char(_) | KeyCode::BackTab) {
        mods |= KeyMods::SHIFT;
    }
    let mut key = Key::new(code);
    key.mods = mods;
    Some(key)
}

/// Whether the host takes the typing for the plugin: it is the canvas's screen (`preview`), the
/// canvas on it asks for text input (`asks`), and `Esc` has not left it since (`left`).
pub fn is_capturing(left: bool, preview: bool, asks: bool) -> bool {
    !left && preview && asks
}

/// Whether `queue` has no more room. The last place is for `Esc` (`esc`): the user's way out is
/// always told to the plugin.
fn is_full(queue: &[Key], esc: bool) -> bool {
    let limit = if esc {
        MAX_QUEUED_KEYS
    } else {
        MAX_QUEUED_KEYS - 1
    };
    queue.len() >= limit
}

/// A key pressed while the plugin takes the typing. `Esc` sets the latch `left` (whatever waits,
/// even when the queue is full) and is queued for the plugin; any other key is queued. Refused
/// with [`Notice::InputFull`] past [`MAX_QUEUED_KEYS`] (the older keys are kept, in order).
pub fn admit_key(queue: &mut Vec<Key>, left: &mut bool, key: Key) -> Admitted {
    let esc = key.code == KeyCode::Esc;
    if esc {
        *left = true;
    }
    if is_full(queue, esc) {
        return Admitted::refused();
    }
    queue.push(key);
    Admitted::queued()
}

/// The bytes of pasted text waiting in `queue`.
pub fn queued_paste_bytes(queue: &[Key]) -> usize {
    queue
        .iter()
        .map(|k| match &k.code {
            KeyCode::Paste(t) => t.len(),
            _ => 0,
        })
        .sum()
}

/// A paste while the plugin takes the typing: one key holding all the text, cut at
/// [`MAX_PASTE_BYTES`] and at what is left of [`MAX_QUEUED_PASTE_BYTES`] (a character boundary;
/// [`Notice::PasteCut`]). Refused when the queue is full or no byte of room is left.
pub fn admit_paste(queue: &mut Vec<Key>, mut text: String) -> Admitted {
    if is_full(queue, false) {
        return Admitted::refused();
    }
    let room = MAX_QUEUED_PASTE_BYTES.saturating_sub(queued_paste_bytes(queue));
    if room == 0 {
        return Admitted::refused();
    }
    let limit = MAX_PASTE_BYTES.min(room);
    let mut notice = None;
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        notice = Some(Notice::PasteCut);
    }
    queue.push(Key::paste(text));
    Admitted {
        queued: true,
        notice,
    }
}

/// A declared action waiting for the session, with its place among the queued keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedAct {
    /// The action id.
    pub action: String,
    /// How many queued keys were pressed before this action and are still waiting: they go first
    /// (the keys and the actions are one line, in the order pressed).
    pub after_keys: usize,
}

impl QueuedAct {
    /// An action with no key queued ahead of it.
    pub fn now(action: &str) -> QueuedAct {
        QueuedAct {
            action: action.to_string(),
            after_keys: 0,
        }
    }
}

/// A declared action pressed while the session is out: queue it behind the `queued_keys` keys
/// that wait. Refused with [`Notice::InputFull`] past [`MAX_QUEUED_ACTS`].
pub fn admit_act(acts: &mut Vec<QueuedAct>, queued_keys: usize, action: &str) -> Admitted {
    if acts.len() >= MAX_QUEUED_ACTS {
        return Admitted::refused();
    }
    acts.push(QueuedAct {
        action: action.to_string(),
        after_keys: queued_keys,
    });
    Admitted::queued()
}

/// The next job for a session that is at hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextJob {
    /// Run the declared action (it has been taken off the line).
    Act(String),
    /// Hand these keys over, in order (they have been taken off the line).
    Keys(Vec<Key>),
}

/// What goes next, **in the order it was pressed**: the oldest queued action when no key was
/// queued before it, else the keys queued ahead of that action (all the keys when no action
/// waits). One job at a time: the host asks again when the answer gives the session back. The
/// count of keys ahead of each action was made when it was pressed; keys cleared since cannot be
/// waited for, hence the clamp.
pub fn next_job(keys: &mut Vec<Key>, acts: &mut Vec<QueuedAct>) -> Option<NextJob> {
    let queued = keys.len();
    let ahead = acts.first().map(|a| a.after_keys.min(queued));
    match ahead {
        Some(0) => Some(NextJob::Act(acts.remove(0).action)),
        _ if queued == 0 => None,
        _ => {
            let n = ahead.unwrap_or(queued);
            for a in acts.iter_mut() {
                a.after_keys = a.after_keys.saturating_sub(n);
            }
            Some(NextJob::Keys(keys.drain(..n).collect()))
        }
    }
}

/// An answer landed with a canvas that asks for text input (`asks`). `by_user` says it answers
/// something the user did. Only such an answer takes the typing back after `Esc` (`left` is
/// cleared); a canvas that asks again by itself (a tick, a refit, a key's answer) does not, so a
/// plugin flapping `text_input` cannot trap the user.
pub fn retake(left: &mut bool, asks: bool, by_user: bool) {
    if asks && by_user {
        *left = false;
    }
}

/// Whether the answer that just landed is the user's. `mark` is "a declared action the user
/// pressed is out" (it is consumed here); `carried_view` says the answer carries a view, and
/// `follows_up` that the action's own answer (without a view) asked for a redraw or a reload,
/// whose answer follows and keeps the mark.
pub fn settle_user_act(mark: &mut bool, carried_view: bool, follows_up: bool) -> bool {
    if carried_view {
        return std::mem::take(mark);
    }
    if !follows_up {
        *mark = false;
    }
    false
}

/// An answer was discarded as stale: the mark goes with it when it is the answer to the user's
/// action or to something asked after it (`gen >= mark_gen`).
pub fn discard_stale(mark: &mut bool, mark_gen: u64, gen: u64) {
    if *mark && gen >= mark_gen {
        *mark = false;
    }
}

/// The show is going (another file or tab): forget the keys, the latch and the mark. The
/// actions live with the show's flow and go with it.
pub fn forget(keys: &mut Vec<Key>, left: &mut bool, mark: &mut bool) {
    keys.clear();
    *left = false;
    *mark = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn ev(code: C, mods: M) -> KeyEvent {
        let mut e = KeyEvent::new(code, mods);
        e.kind = KeyEventKind::Press;
        e
    }

    fn keys(n: usize) -> Vec<Key> {
        (0..n).map(|_| Key::char('a')).collect()
    }

    // --- the key a plugin receives -----------------------------------------------------------

    #[test]
    fn a_key_is_named_with_its_modifiers_and_shift_is_in_the_character() {
        assert_eq!(key_of(&ev(C::Char('a'), M::NONE)), Some(Key::char('a')));
        assert_eq!(key_of(&ev(C::Char('A'), M::SHIFT)), Some(Key::char('A')));
        assert_eq!(key_of(&ev(C::Char('c'), M::CONTROL)), Some(Key::ctrl('c')));
        assert_eq!(
            key_of(&ev(C::Char('x'), M::ALT)),
            Some(Key::char('x').with_mods(KeyMods::ALT))
        );
        assert_eq!(
            key_of(&ev(C::BackTab, M::SHIFT)),
            Some(Key::new(KeyCode::BackTab))
        );
        assert_eq!(
            key_of(&ev(C::Left, M::SHIFT)),
            Some(Key::new(KeyCode::Left).with_mods(KeyMods::SHIFT))
        );
        assert_eq!(
            key_of(&ev(C::F(13), M::NONE)),
            Some(Key::new(KeyCode::F(13)))
        );
        assert_eq!(key_of(&ev(C::Esc, M::NONE)), Some(Key::new(KeyCode::Esc)));
    }

    #[test]
    fn a_key_without_a_plugin_name_is_none() {
        assert_eq!(key_of(&ev(C::Insert, M::NONE)), None);
        assert_eq!(key_of(&ev(C::CapsLock, M::NONE)), None);
        assert_eq!(key_of(&ev(C::Null, M::NONE)), None);
    }

    // --- when the host takes the typing ------------------------------------------------------

    #[test]
    fn the_typing_is_taken_only_while_all_three_hold() {
        for left in [false, true] {
            for preview in [false, true] {
                for asks in [false, true] {
                    assert_eq!(
                        is_capturing(left, preview, asks),
                        !left && preview && asks,
                        "left={left} preview={preview} asks={asks}"
                    );
                }
            }
        }
    }

    // --- Esc leaves, and is always told ------------------------------------------------------

    #[test]
    fn a_key_is_queued_in_order_and_esc_sets_the_latch() {
        let mut q = Vec::new();
        let mut left = false;
        let a = admit_key(&mut q, &mut left, Key::char('a'));
        assert_eq!(
            a,
            Admitted {
                queued: true,
                notice: None
            }
        );
        assert!(!left, "a plain key does not leave");
        admit_key(&mut q, &mut left, Key::char('b'));
        assert!(!left);
        admit_key(&mut q, &mut left, Key::new(KeyCode::Esc));
        assert!(left, "Esc leaves");
        assert_eq!(
            q,
            vec![Key::char('a'), Key::char('b'), Key::new(KeyCode::Esc)]
        );
    }

    #[test]
    fn the_last_place_of_the_queue_is_kept_for_esc() {
        let mut q = keys(MAX_QUEUED_KEYS - 1);
        let mut left = false;
        let a = admit_key(&mut q, &mut left, Key::char('x'));
        assert_eq!(a.notice, Some(Notice::InputFull));
        assert!(!a.queued);
        assert_eq!(
            q.len(),
            MAX_QUEUED_KEYS - 1,
            "the older keys stay as they are"
        );
        assert!(!left);
        let e = admit_key(&mut q, &mut left, Key::new(KeyCode::Esc));
        assert!(e.queued, "Esc takes the last place");
        assert!(left);
        assert_eq!(q.len(), MAX_QUEUED_KEYS);
        assert_eq!(q.last(), Some(&Key::new(KeyCode::Esc)));
    }

    #[test]
    fn esc_leaves_even_when_the_queue_is_completely_full() {
        let mut q = keys(MAX_QUEUED_KEYS);
        let mut left = false;
        let e = admit_key(&mut q, &mut left, Key::new(KeyCode::Esc));
        assert!(!e.queued);
        assert_eq!(e.notice, Some(Notice::InputFull));
        assert!(left, "the user's way out is the host's: the latch is set");
        assert_eq!(q.len(), MAX_QUEUED_KEYS);
    }

    // --- pastes -------------------------------------------------------------------------------

    #[test]
    fn a_paste_is_one_key_with_all_the_text() {
        let mut q = Vec::new();
        let a = admit_paste(&mut q, "héllo\nworld".to_string());
        assert!(a.queued && a.notice.is_none());
        assert_eq!(q, vec![Key::paste("héllo\nworld")]);
    }

    #[test]
    fn a_long_paste_is_cut_at_a_character_boundary_with_a_notice() {
        // Three-byte characters never line up with the limit.
        let text = "あ".repeat(MAX_PASTE_BYTES / 3 + 10);
        assert!(text.len() > MAX_PASTE_BYTES);
        let mut q = Vec::new();
        let a = admit_paste(&mut q, text);
        assert!(a.queued);
        assert_eq!(a.notice, Some(Notice::PasteCut));
        let KeyCode::Paste(t) = &q[0].code else {
            panic!("a paste");
        };
        assert!(t.len() <= MAX_PASTE_BYTES && t.len() > MAX_PASTE_BYTES - 3);
        assert!(t.chars().all(|c| c == 'あ'));
    }

    #[test]
    fn a_paste_exactly_at_the_limit_is_not_cut() {
        let mut q = Vec::new();
        let a = admit_paste(&mut q, "a".repeat(MAX_PASTE_BYTES));
        assert!(a.queued && a.notice.is_none());
    }

    #[test]
    fn the_pastes_together_are_bounded_and_the_last_is_cut_to_what_is_left() {
        let mut q = Vec::new();
        let fits = MAX_QUEUED_PASTE_BYTES / MAX_PASTE_BYTES;
        for _ in 0..fits {
            let a = admit_paste(&mut q, "a".repeat(MAX_PASTE_BYTES));
            assert!(a.queued && a.notice.is_none());
        }
        assert_eq!(queued_paste_bytes(&q), MAX_QUEUED_PASTE_BYTES);
        let a = admit_paste(&mut q, "b".to_string());
        assert!(!a.queued, "no byte of room is left");
        assert_eq!(a.notice, Some(Notice::InputFull));
        assert_eq!(q.len(), fits);
    }

    #[test]
    fn a_paste_is_cut_to_the_room_that_remains() {
        let mut q = vec![Key::paste("a".repeat(MAX_QUEUED_PASTE_BYTES - 10))];
        let a = admit_paste(&mut q, "b".repeat(100));
        assert!(a.queued);
        assert_eq!(a.notice, Some(Notice::PasteCut));
        let KeyCode::Paste(t) = &q[1].code else {
            panic!("a paste");
        };
        assert_eq!(t.len(), 10);
        assert_eq!(queued_paste_bytes(&q), MAX_QUEUED_PASTE_BYTES);
    }

    #[test]
    fn typed_keys_do_not_count_as_pasted_bytes() {
        assert_eq!(queued_paste_bytes(&keys(100)), 0);
    }

    #[test]
    fn a_paste_is_refused_when_the_key_queue_has_no_room() {
        let mut q = keys(MAX_QUEUED_KEYS - 1);
        let a = admit_paste(&mut q, "x".to_string());
        assert!(!a.queued);
        assert_eq!(a.notice, Some(Notice::InputFull));
        assert_eq!(q.len(), MAX_QUEUED_KEYS - 1);
    }

    // --- declared actions in the line --------------------------------------------------------

    #[test]
    fn an_action_remembers_how_many_keys_were_ahead_of_it() {
        let mut acts = Vec::new();
        let a = admit_act(&mut acts, 3, "go");
        assert!(a.queued && a.notice.is_none());
        assert_eq!(
            acts,
            vec![QueuedAct {
                action: "go".into(),
                after_keys: 3
            }]
        );
    }

    #[test]
    fn the_action_line_is_bounded_and_refuses_in_order() {
        let mut acts = Vec::new();
        for i in 0..MAX_QUEUED_ACTS {
            assert!(admit_act(&mut acts, 0, &format!("a{i}")).queued);
        }
        let a = admit_act(&mut acts, 0, "late");
        assert!(!a.queued);
        assert_eq!(a.notice, Some(Notice::InputFull));
        assert_eq!(acts.len(), MAX_QUEUED_ACTS);
        assert_eq!(acts[0].action, "a0");
        assert!(acts.iter().all(|a| a.action != "late"));
    }

    // --- the line: keys and actions in the order pressed -------------------------------------

    #[test]
    fn nothing_waiting_is_no_job() {
        assert_eq!(next_job(&mut Vec::new(), &mut Vec::new()), None);
    }

    #[test]
    fn keys_alone_go_together_in_order() {
        let mut k = vec![Key::char('a'), Key::char('b')];
        let job = next_job(&mut k, &mut Vec::new());
        assert_eq!(
            job,
            Some(NextJob::Keys(vec![Key::char('a'), Key::char('b')]))
        );
        assert!(k.is_empty());
    }

    #[test]
    fn an_action_with_no_key_ahead_goes_first_and_alone() {
        let mut k = vec![Key::char('a')];
        let mut acts = vec![QueuedAct::now("go")];
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Act("go".to_string()))
        );
        assert!(acts.is_empty());
        assert_eq!(k.len(), 1, "the key after it still waits");
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Keys(vec![Key::char('a')]))
        );
    }

    #[test]
    fn the_keys_ahead_of_an_action_go_before_it() {
        // a b [act1] c [act2] d : pressed in this order.
        let mut k = vec![
            Key::char('a'),
            Key::char('b'),
            Key::char('c'),
            Key::char('d'),
        ];
        let mut acts = vec![
            QueuedAct {
                action: "act1".into(),
                after_keys: 2,
            },
            QueuedAct {
                action: "act2".into(),
                after_keys: 3,
            },
        ];
        let mut order = Vec::new();
        while let Some(job) = next_job(&mut k, &mut acts) {
            order.push(match job {
                NextJob::Act(a) => a,
                NextJob::Keys(ks) => ks
                    .iter()
                    .map(|k| match k.code {
                        KeyCode::Char(c) => c.to_string(),
                        _ => "?".into(),
                    })
                    .collect::<String>(),
            });
        }
        assert_eq!(order, ["ab", "act1", "c", "act2", "d"]);
    }

    #[test]
    fn keys_cleared_since_cannot_be_waited_for() {
        // The action counted 5 keys ahead, but only 1 is left: it is not stuck behind keys that
        // are gone.
        let mut k = vec![Key::char('a')];
        let mut acts = vec![QueuedAct {
            action: "go".into(),
            after_keys: 5,
        }];
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Keys(vec![Key::char('a')]))
        );
        assert_eq!(
            acts[0].after_keys, 4,
            "the keys that went are taken off the count"
        );
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Act("go".to_string()))
        );
    }

    #[test]
    fn actions_alone_go_one_at_a_time() {
        let mut acts = vec![QueuedAct::now("one"), QueuedAct::now("two")];
        let mut k = Vec::new();
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Act("one".to_string()))
        );
        assert_eq!(
            next_job(&mut k, &mut acts),
            Some(NextJob::Act("two".to_string()))
        );
        assert_eq!(next_job(&mut k, &mut acts), None);
    }

    // --- taking the typing back --------------------------------------------------------------

    #[test]
    fn only_the_users_answer_that_asks_takes_the_typing_back() {
        for asks in [false, true] {
            for by_user in [false, true] {
                let mut left = true;
                retake(&mut left, asks, by_user);
                assert_eq!(left, !(asks && by_user), "asks={asks} by_user={by_user}");
            }
        }
        let mut left = false;
        retake(&mut left, false, false);
        assert!(!left, "never sets the latch");
    }

    #[test]
    fn the_mark_goes_with_the_view_that_carries_it() {
        let mut mark = true;
        assert!(settle_user_act(&mut mark, true, false));
        assert!(!mark, "taken");
        let mut mark = false;
        assert!(!settle_user_act(&mut mark, true, false));
    }

    #[test]
    fn an_answer_without_a_view_keeps_the_mark_only_when_a_follow_up_comes() {
        let mut mark = true;
        assert!(!settle_user_act(&mut mark, false, true));
        assert!(
            mark,
            "a redraw or a reload follows: its answer is the user's"
        );
        assert!(!settle_user_act(&mut mark, false, false));
        assert!(!mark, "nothing follows: the mark is dropped");
    }

    #[test]
    fn a_stale_answer_takes_the_mark_only_if_it_is_the_actions_or_later() {
        let mut mark = true;
        discard_stale(&mut mark, 10, 9);
        assert!(mark, "an older answer is not the action's");
        discard_stale(&mut mark, 10, 10);
        assert!(!mark);
        let mut mark = true;
        discard_stale(&mut mark, 10, 11);
        assert!(!mark, "asked after it");
        let mut mark = false;
        discard_stale(&mut mark, 10, 11);
        assert!(!mark);
    }

    #[test]
    fn forgetting_clears_the_keys_the_latch_and_the_mark() {
        let mut k = keys(3);
        let (mut left, mut mark) = (true, true);
        forget(&mut k, &mut left, &mut mark);
        assert!(k.is_empty() && !left && !mark);
    }
}
