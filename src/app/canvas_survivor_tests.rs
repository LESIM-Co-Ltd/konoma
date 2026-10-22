//! Tests added after the stage-2.5 mutation audit (axis 4), batch E: rules of the canvas road that
//! need state no key sequence reaches (a show that is not ready, a show whose file is not the one
//! on screen, a work group that is busy with a job of its own). They set that state directly, on
//! an `App` driven through `handle_key` and a `TestBackend`, as the end-to-end tests do.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_plugin::prelude::*;
use konoma_plugin::view::CanvasView;
use konoma_plugin::Key;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::plugin_host::PluginPhase;
use super::*;
use crate::config::Config;
use crate::plugins::test_plugins;
use crate::test_support::unique_tmp;

/// What the stand-in saw.
#[derive(Default)]
struct Seen {
    keys: Mutex<Vec<Key>>,
    views: AtomicUsize,
    /// The declared actions that ran, in order.
    acts: Mutex<Vec<String>>,
    /// The thread each `view` ran on.
    view_threads: Mutex<Vec<std::thread::ThreadId>>,
    /// `CanvasView::redraw_after` of the next canvas.
    after: Mutex<Option<std::time::Duration>>,
    /// Every key (`key:a`) and every action (`act:one`) the session got, in the order it got them.
    order: Mutex<Vec<String>>,
    /// The request of every `view`.
    reqs: Mutex<Vec<ViewReq>>,
}

impl Seen {
    fn typed(&self) -> String {
        self.keys
            .lock()
            .unwrap()
            .iter()
            .map(|k| match &k.code {
                konoma_plugin::key::KeyCode::Char(c) => c.to_string(),
                other => format!("<{other:?}>"),
            })
            .collect()
    }
}

/// A canvas that takes the typing and records the keys it is given. With a `group` it declares an
/// exclusive work group (as the Office plugins do).
struct Mini {
    id: String,
    group: Option<String>,
    seen: Arc<Seen>,
    /// Also declares a table and reports separate pages (a plugin of both display types).
    table: bool,
}

impl Plugin for Mini {
    fn manifest(&self) -> Manifest {
        let m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.mini"]))
            .shows(ViewKind::Canvas)
            .action(Act::new("one").key("x").label("one").help("The first"))
            .action(Act::new("two").key("z").label("two").help("The second"));
        let m = if self.table {
            m.shows(ViewKind::Table)
        } else {
            m
        };
        match &self.group {
            Some(g) => m.work_group(g.as_str()),
            None => m,
        }
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(MiniSession {
            seen: self.seen.clone(),
            table: self.table,
        }))
    }
}

struct MiniSession {
    seen: Arc<Seen>,
    table: bool,
}

impl Session for MiniSession {
    fn layout(&self) -> konoma_plugin::view::Layout {
        let mut l = konoma_plugin::view::Layout::single();
        if self.table {
            l.pages = Some(konoma_plugin::view::PageLayout::Separate {
                count: Some(3),
                titles: Vec::new(),
            });
        }
        l
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.seen.views.fetch_add(1, Ordering::SeqCst);
        self.seen.reqs.lock().unwrap().push(req.clone());
        self.seen
            .view_threads
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        let mut c = CanvasView::new(req.cols, req.rows);
        c.set_string(0, 0, "mini", konoma_plugin::Style::new());
        c.text_input = true;
        c.redraw_after = *self.seen.after.lock().unwrap();
        Ok(View::Canvas(c))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.seen.acts.lock().unwrap().push(action.to_string());
        self.seen
            .order
            .lock()
            .unwrap()
            .push(format!("act:{action}"));
        Ok(Outcome::Unhandled)
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        self.seen.keys.lock().unwrap().push(key.clone());
        if let konoma_plugin::key::KeyCode::Char(c) = &key.code {
            self.seen.order.lock().unwrap().push(format!("key:{c}"));
        }
        Ok(Outcome::Unhandled)
    }
}

struct H {
    app: App,
    term: Terminal<TestBackend>,
    rx: Option<Receiver<MediaResult>>,
    seen: Arc<Seen>,
    dir: crate::test_support::TmpDir,
}

impl H {
    /// An app previewing `a.mini` with the stand-in, drawn once. With `asynchronous` the plugin
    /// jobs run on workers (the one channel of the run loop), as in the real program.
    fn open(name: &str, group: Option<&str>, asynchronous: bool) -> H {
        H::open_as(name, group, asynchronous, false)
    }

    /// [`H::open`] with the plugin also declaring a table (`table`) and reporting pages.
    fn open_as(name: &str, group: Option<&str>, asynchronous: bool, table: bool) -> H {
        let dir = unique_tmp(&format!("konoma_canvas_surv_{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.mini"), b"x").unwrap();
        std::fs::write(dir.join("b.mini"), b"y").unwrap();
        let seen = Arc::new(Seen::default());
        let marker = test_plugins::register(
            name,
            Arc::new(Mini {
                id: name.to_string(),
                group: group.map(str::to_string),
                seen: seen.clone(),
                table,
            }),
        );
        let cfg: Config = test_plugins::marker_config(&marker);
        let mut app = App::new(dir.canonicalize().unwrap(), cfg).expect("App::new");
        let mut rx = None;
        if asynchronous {
            let (tx, r) = channel();
            app.attach_media_loader(tx);
            rx = Some(r);
        }
        let term = Terminal::new(TestBackend::new(90, 26)).expect("terminal");
        let mut h = H {
            app,
            term,
            rx,
            seen,
            dir,
        };
        h.draw();
        let i = h
            .app
            .tab
            .entries
            .iter()
            .position(|e| e.path.to_string_lossy().ends_with("a.mini"))
            .expect("a.mini is listed");
        h.app.tab.selected = i;
        h.press(KeyCode::Enter);
        h.settle();
        // The first frame measured the area and asked the canvas again; draw the answer.
        h.draw();
        h.settle();
        h.draw();
        h
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| crate::ui::render(f, app)).expect("draw");
    }

    fn press(&mut self, code: KeyCode) {
        crate::handle_key(&mut self.app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
        self.draw();
    }

    /// Apply the answers of the workers until no job is out.
    #[track_caller]
    fn settle(&mut self) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        for _ in 0..20 {
            if !self.app.is_media_loading() {
                return;
            }
            let res = rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("a worker answers");
            self.app.apply_media(res);
        }
        panic!("a plugin job is still out after 20 answers");
    }

    fn show(&self) -> &plugin_host::PluginShow {
        self.app.plugin.as_ref().expect("a plugin show")
    }

    fn show_mut(&mut self) -> &mut plugin_host::PluginShow {
        self.app.plugin.as_mut().expect("a plugin show")
    }
}

// ---- is_canvas_preview: a show that is stale for the screen is not the canvas on screen ------------

#[test]
fn a_canvas_show_that_is_ready_for_the_file_on_screen_is_the_canvas_preview() {
    let h = H::open("cs_ok", None, false);
    assert!(h.app.is_canvas_preview());
    assert!(h.app.canvas_view().is_some());
    assert!(h.app.is_plugin_input());
}

/// A show that is not ready does not count, whatever view it still holds.
#[test]
fn a_canvas_show_that_is_not_ready_is_not_the_canvas_preview() {
    let mut h = H::open("cs_phase", None, false);
    assert!(h.show().view.is_some());
    h.show_mut().phase = PluginPhase::Loading;
    assert!(h.show().view.is_some(), "the view is still there");
    assert!(!h.app.is_canvas_preview(), "loading");
    assert!(h.app.canvas_view().is_none());
    assert!(!h.app.is_plugin_input(), "no typing is taken for it");
    h.show_mut().phase = PluginPhase::Failed(Failure::external(Tr::new("broken")));
    assert!(!h.app.is_canvas_preview(), "failed");
    assert!(!h.app.is_plugin_input());
    h.show_mut().phase = PluginPhase::Ready;
    assert!(h.app.is_canvas_preview(), "ready again");
}

/// A show opened for another file of the same plugin (the preview moved on) does not count.
#[test]
fn a_canvas_show_of_another_file_is_not_the_canvas_preview() {
    let mut h = H::open("cs_file", None, false);
    let Some(PreviewKind::Plugin { id, path, hint }) = h.app.tab.preview_kind.clone() else {
        panic!("a plugin preview");
    };
    h.app.tab.preview_kind = Some(PreviewKind::Plugin {
        path: h.dir.join("b.mini"),
        id: id.clone(),
        hint: hint.clone(),
    });
    assert!(!h.app.is_canvas_preview(), "the show is for a.mini");
    assert!(h.app.canvas_view().is_none());
    assert!(!h.app.is_plugin_input());
    // The same file under another plugin id is not it either.
    h.app.tab.preview_kind = Some(PreviewKind::Plugin {
        path: path.clone(),
        id: "someone-else".into(),
        hint: hint.clone(),
    });
    assert!(!h.app.is_canvas_preview());
    h.app.tab.preview_kind = Some(PreviewKind::Plugin { path, id, hint });
    assert!(
        h.app.is_canvas_preview(),
        "back to the file the show is for"
    );
}

/// Only a preview of a plugin's file can be a canvas preview. A delegated command also "shows its
/// file" (`shows_plugin_file`) and is never a canvas, whatever the show it left behind holds.
#[test]
fn a_command_preview_is_not_a_canvas_preview() {
    let mut h = H::open("cs_kind", None, false);
    let path = h.show().path.clone();
    h.show_mut().id = crate::plugins::COMMAND_ID.to_string();
    h.app.tab.preview_kind = Some(PreviewKind::Command {
        path,
        template: "true".into(),
        render_as: None,
        detached: false,
    });
    let show = h.show();
    assert!(
        h.app.shows_plugin_file(show),
        "the command show is for the file on screen"
    );
    assert!(!h.app.is_canvas_preview());
    assert!(h.app.canvas_view().is_none());
    // And a text preview, a git diff and a "can not preview" screen are not canvases either.
    for kind in [
        Some(PreviewKind::CanNotPreview { ext: "mini".into() }),
        Some(PreviewKind::Text(h.show().path.clone())),
        None,
    ] {
        h.app.tab.preview_kind = kind;
        assert!(!h.app.is_canvas_preview());
    }
}

/// The deadline of the redraw timer and the poll timeout follow `is_canvas_preview`: a stale show
/// asks for nothing and the deadline it left is dropped.
#[test]
fn a_stale_show_has_no_redraw_timer() {
    let mut h = H::open("cs_timer", None, false);
    let at = std::time::Instant::now();
    h.app.set_canvas_redraw_at_for_test(Some(at));
    assert!(
        h.app.canvas_poll_timeout(at).is_some(),
        "the canvas is on screen"
    );
    h.show_mut().phase = PluginPhase::Loading;
    assert_eq!(h.app.canvas_poll_timeout(at), None);
    let views = h.seen.views.load(Ordering::SeqCst);
    assert!(!h
        .app
        .canvas_redraw_if_due(at + std::time::Duration::from_secs(5)));
    assert_eq!(
        h.seen.views.load(Ordering::SeqCst),
        views,
        "nobody was asked"
    );
    assert_eq!(
        h.app.canvas_redraw_deadline(),
        None,
        "and the deadline is dropped"
    );
}

// ---- the keys waiting for the session go only when nothing keeps a job from being sent ------------

fn waiting_key(c: char) -> Key {
    Key::new(konoma_plugin::key::KeyCode::Char(c))
}

/// A show that holds its session but is not ready (a reload that failed leaves one, a page being
/// read has one): the keys wait, are not lost, and go once it is ready again.
#[test]
fn keys_wait_while_the_show_is_not_ready_and_go_when_it_is() {
    let mut h = H::open("cs_flush_phase", None, true);
    assert!(h.show().session.is_some());
    h.show_mut().phase = PluginPhase::Loading;
    h.app.plugin_key_queue.push(waiting_key('a'));
    h.app.flush_plugin_keys();
    assert_eq!(h.app.plugin_key_queue.len(), 1, "the key still waits");
    assert!(h.show().session.is_some(), "no job took the session");
    assert!(!h.app.is_media_loading(), "and none was started");
    assert_eq!(h.seen.typed(), "");
    h.show_mut().phase = PluginPhase::Ready;
    h.app.flush_plugin_keys();
    assert!(h.app.plugin_key_queue.is_empty());
    h.settle();
    assert_eq!(h.seen.typed(), "a", "delivered once the show was ready");
}

/// A plugin in an exclusive work group: while the group runs a job, a key must not be sent (a job
/// parked in the group's slot could be replaced by a newer one, and keys are never replaced). It
/// waits, in order, and goes when the group is free.
#[test]
fn keys_wait_while_the_plugins_work_group_is_busy_and_go_when_it_is_free() {
    let mut h = H::open("cs_flush_group", Some("cs-group"), true);
    assert!(h.show().session.is_some());
    assert!(!h.app.is_media_loading());
    // Another job of the group is running.
    h.app
        .plugin_groups
        .entry("cs-group".into())
        .or_default()
        .busy = true;
    let dispatched = h.app.plugin_group_dispatches;
    // The user types: the key is queued, and nothing is sent.
    h.press(KeyCode::Char('a'));
    h.press(KeyCode::Char('b'));
    assert_eq!(h.app.plugin_key_queue.len(), 2, "both keys wait");
    assert!(h.show().session.is_some(), "no job took the session");
    assert_eq!(
        h.app.plugin_group_dispatches, dispatched,
        "no job was dispatched"
    );
    assert!(
        h.app.plugin_groups["cs-group"].queued.is_none(),
        "and none was parked in the group's slot, where a newer job would replace it"
    );
    assert_eq!(h.seen.typed(), "");
    // The group is free again: the keys go, in order, in one job.
    h.app.plugin_groups.get_mut("cs-group").unwrap().busy = false;
    h.app.flush_plugin_keys();
    assert!(h.app.plugin_key_queue.is_empty());
    h.settle();
    assert_eq!(h.seen.typed(), "ab");
}

/// The control of the test above: the same plugin in no group sends the key at once.
#[test]
fn keys_of_a_plugin_without_a_work_group_are_sent_at_once() {
    let mut h = H::open("cs_flush_nogroup", None, true);
    h.press(KeyCode::Char('a'));
    assert!(h.app.plugin_key_queue.is_empty());
    h.settle();
    assert_eq!(h.seen.typed(), "a");
}

// ---- the surface of the text input layer -------------------------------------------------------

#[test]
fn the_text_input_surface_sits_on_the_text_input_layer() {
    use crate::keymap::Surface;
    assert!(matches!(Surface::PluginInput.layer(), UiLayer::PluginInput));
    assert_eq!(UiLayer::PluginInput.surface(), Some(Surface::PluginInput));
    // A canvas that takes the typing is the frontmost layer of the app that shows it.
    let h = H::open("cs_surface", None, false);
    assert_eq!(h.app.surface(), Surface::PluginInput);
    assert!(matches!(
        h.app.frontmost_layer(|_| true),
        UiLayer::PluginInput
    ));
}

// ---- batch F: queued actions, the user mark, the paste backlog, the reclaimed timer --------------------

impl Seen {
    fn acts(&self) -> Vec<String> {
        self.acts.lock().unwrap().clone()
    }
}

/// A press never overtakes the ones before it: with presses waiting, a new press goes behind them
/// even when the session is at hand. (Before: the new press ran at once and the older ones were
/// run after it.)
#[test]
fn an_action_pressed_with_older_presses_waiting_goes_behind_them() {
    let mut h = H::open("cs_acts_order", None, true);
    assert!(h.show().session.is_some());
    h.show_mut()
        .flow
        .acts
        .push(plugin_host::QueuedAct::now("one"));
    assert!(h.app.plugin_act("two"));
    h.settle();
    assert_eq!(h.seen.acts(), ["one", "two"], "in the order pressed");
    assert!(h.show().flow.acts.is_empty());
}

/// Queued actions are flushed wherever queued keys are: when the plugin's work group is freed by
/// the result of another job, `apply_media` gives them the session.
#[test]
fn queued_actions_go_when_a_work_group_is_freed_by_another_jobs_result() {
    let mut h = H::open("cs_acts_group", Some("cs-acts-group"), true);
    h.app
        .plugin_groups
        .entry("cs-acts-group".into())
        .or_default()
        .busy = true;
    let dispatched = h.app.plugin_group_dispatches;
    assert!(h.app.plugin_act("one"));
    assert!(h.app.plugin_act("two"));
    assert_eq!(
        h.show()
            .flow
            .acts
            .iter()
            .map(|a| a.action.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"],
        "both wait"
    );
    assert_eq!(
        h.app.plugin_group_dispatches, dispatched,
        "nothing was sent"
    );
    assert!(h.seen.acts().is_empty());
    // The group is free again, through a result that is not this plugin's own answer: any result
    // reaches `apply_media`.
    h.app.plugin_groups.get_mut("cs-acts-group").unwrap().busy = false;
    let stale_gen = h.app.media_gen.wrapping_add(7);
    h.app.apply_media(MediaResult {
        gen: stale_gen,
        payload: None,
    });
    h.settle();
    assert_eq!(h.seen.acts(), ["one", "two"], "flushed, in order");
}

fn stale_result(h: &H, gen: u64) -> MediaResult {
    let show = h.show();
    let mut res = plugin_host::PluginResult::failed(
        gen,
        show.id.clone(),
        show.req.clone(),
        Failure::external("stale"),
    );
    res.path = show.path.clone();
    MediaResult {
        gen,
        payload: Some(MediaPayload::Plugin(Box::new(res))),
    }
}

/// The mark "this answer is the user's" goes with the answer that is discarded as stale, so a
/// later answer to something else is not taken for the answer to the action.
#[test]
fn a_discarded_stale_answer_takes_the_user_mark_away() {
    let mut h = H::open("cs_mark", None, true);
    let mine = h.app.media_gen;
    h.app.plugin_user_act = true;
    h.app.plugin_user_act_gen = mine;
    // An answer older than the action says nothing about it.
    h.app.apply_media(stale_result(&h, mine.wrapping_sub(1)));
    assert!(h.app.plugin_user_act, "an older answer keeps the mark");
    // The action's own answer, or a later one, discarded: the mark goes.
    h.app.apply_media(stale_result(&h, mine.wrapping_add(1)));
    assert!(!h.app.plugin_user_act);
    // The same through the direct path (a result that does not belong).
    h.app.plugin_user_act = true;
    h.app.plugin_user_act_gen = mine;
    h.app.discard_stale_answer(mine);
    assert!(!h.app.plugin_user_act);
}

/// A stale answer that gives the session back re-arms the redraw timer like a landed answer.
#[test]
fn a_session_given_back_by_a_stale_answer_arms_the_redraw_timer_again() {
    let mut h = H::open("cs_reclaim_timer", None, true);
    *h.seen.after.lock().unwrap() = Some(std::time::Duration::from_millis(500));
    // Draw once more so the show holds a canvas that asks for the timer.
    h.app.canvas_redraw_if_due(std::time::Instant::now());
    h.app.plugin_request_view(h.app.plugin_view_req());
    h.settle();
    assert!(
        h.app.canvas_redraw_deadline().is_some(),
        "armed by the landing"
    );
    // A tick was skipped while a job was out: the deadline is gone, the session is out too.
    h.app.set_canvas_redraw_at_for_test(None);
    let session = h.show_mut().session.take().expect("a session");
    let mut stale = stale_result(&h, h.app.media_gen.wrapping_add(1));
    if let Some(MediaPayload::Plugin(res)) = stale.payload.as_mut() {
        res.session = Some(session);
    }
    h.app.apply_media(stale);
    assert!(h.show().session.is_some(), "the session is back");
    assert!(
        h.app.canvas_redraw_deadline().is_some(),
        "the animation goes on"
    );
}

/// The synchronous `view` (on the UI thread) is for the host's own standard document plugins only:
/// a plugin a user added, even one that declares a document too, is asked on a worker.
#[test]
fn a_plugin_that_is_not_a_standard_one_is_never_asked_for_a_view_on_the_ui_thread() {
    let mut h = H::open("cs_sync_view", None, true);
    assert!(!crate::plugins::profile_of("cs_sync_view").standard);
    let me = std::thread::current().id();
    let before = h.seen.view_threads.lock().unwrap().len();
    let req = h.app.plugin_view_req();
    assert!(
        h.app.plugin_view_now(req.clone()),
        "the answer is on its way"
    );
    h.settle();
    let threads = h.seen.view_threads.lock().unwrap()[before..].to_vec();
    assert!(!threads.is_empty(), "the view was asked for");
    assert!(threads.iter().all(|t| *t != me), "and not on this thread");
    // With a job holding the session there is nothing to ask: false, as for a standard one.
    let held = h.show_mut().session.take();
    assert!(!h.app.plugin_view_now(req));
    h.show_mut().session = held;
}

#[test]
fn only_the_hosts_own_plugins_are_standard() {
    for id in ["markdown", "document", "presentation", "code", "csv"] {
        assert!(crate::plugins::profile_of(id).standard, "{id}");
    }
    assert!(!crate::plugins::profile_of("no-such-plugin").standard);
}

// ---- round 3: one line for keys and actions, the first answer of a session, the surface -------------------

impl H {
    /// Free the work group `g` through a result that is not this plugin's (as a job of another
    /// plugin of the group would), so the queue goes.
    fn free_group(&mut self, g: &str) {
        self.app.plugin_groups.get_mut(g).unwrap().busy = false;
        let stale_gen = self.app.media_gen.wrapping_add(7);
        self.app.apply_media(MediaResult {
            gen: stale_gen,
            payload: None,
        });
        self.settle();
    }
}

fn typed_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Keys and actions are one line: what was pressed first goes first, whatever kind it is.
#[test]
fn keys_and_actions_pressed_while_the_group_is_busy_go_in_the_order_pressed() {
    let mut h = H::open("cs_one_line", Some("cs-one-line"), true);
    h.app
        .plugin_groups
        .entry("cs-one-line".into())
        .or_default()
        .busy = true;
    // An action first, then typing, then another action, then more typing.
    assert!(h.app.plugin_act("one"));
    h.app.plugin_input_key(&typed_key('a'));
    h.app.plugin_input_key(&typed_key('b'));
    assert!(h.app.plugin_act("two"));
    h.app.plugin_input_key(&typed_key('c'));
    assert!(h.seen.order.lock().unwrap().is_empty(), "nothing was sent");
    h.free_group("cs-one-line");
    assert_eq!(
        *h.seen.order.lock().unwrap(),
        ["act:one", "key:a", "key:b", "act:two", "key:c"],
        "the order of the presses"
    );
    assert!(h.app.plugin_key_queue.is_empty());
    assert!(h.show().flow.acts.is_empty());
}

/// Typing first and an action after it: the keys are not held back by the action, and the action
/// does not go ahead of them.
#[test]
fn typing_pressed_before_an_action_goes_before_it() {
    let mut h = H::open("cs_one_line2", Some("cs-one-line2"), true);
    h.app
        .plugin_groups
        .entry("cs-one-line2".into())
        .or_default()
        .busy = true;
    h.app.plugin_input_key(&typed_key('a'));
    assert!(h.app.plugin_act("one"));
    // The action recorded how many keys were ahead of it.
    assert_eq!(h.show().flow.acts[0].after_keys, 1);
    h.free_group("cs-one-line2");
    assert_eq!(*h.seen.order.lock().unwrap(), ["key:a", "act:one"]);
}

/// An action pressed with keys waiting never overtakes them (the keys wait for a busy group).
#[test]
fn an_action_never_overtakes_keys_that_wait() {
    let mut h = H::open("cs_one_line3", Some("cs-one-line3"), true);
    h.app
        .plugin_groups
        .entry("cs-one-line3".into())
        .or_default()
        .busy = true;
    h.app.plugin_key_queue.push(waiting_key('a'));
    assert!(h.app.plugin_act("one"));
    assert_eq!(h.show().flow.acts.len(), 1, "queued behind the key");
    assert_eq!(h.show().flow.acts[0].after_keys, 1);
    h.free_group("cs-one-line3");
    assert_eq!(*h.seen.order.lock().unwrap(), ["key:a", "act:one"]);
}

/// Keys cleared (the show forgot them) cannot be waited for: an action that counted them is not
/// stuck behind keys that are gone.
#[test]
fn an_action_that_counted_keys_which_are_gone_still_goes() {
    let mut h = H::open("cs_one_line4", Some("cs-one-line4"), true);
    h.app
        .plugin_groups
        .entry("cs-one-line4".into())
        .or_default()
        .busy = true;
    h.app.plugin_key_queue.push(waiting_key('a'));
    assert!(h.app.plugin_act("one"));
    h.app.plugin_key_queue.clear();
    h.free_group("cs-one-line4");
    assert_eq!(*h.seen.order.lock().unwrap(), ["act:one"]);
}

/// The first answer of a new session must not resolve `unchanged` to the pictures of the session
/// before it (the file was read again with the old view kept).
#[test]
fn the_open_of_a_new_session_is_not_told_the_old_sessions_ids_or_kept_sources() {
    use super::canvas_images::{CanvasPlace, KeptPicture};
    let mut h = H::open("cs_known", None, false);
    h.app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (tx, _rx) = channel();
    h.app.attach_md_encoder(tx);
    let key: Arc<str> = Arc::from("canvas://cs_known/3/00000000000000d1");
    h.app.canvas_places.push(CanvasPlace {
        id: 3,
        rect: konoma_plugin::view::CellRect::new(0, 0, 1, 1),
        key: Some(key.clone()),
    });
    h.app.canvas_front.insert(3, key.clone());
    h.app.canvas_kept.insert(
        3,
        KeptPicture {
            key: Some(key.clone()),
            hash: Some(0xd1),
            data: Arc::new(konoma_plugin::view::ImageData::Rgba {
                width: 1,
                height: 1,
                pixels: Arc::new(vec![0; 4]),
            }),
            needs: None,
        },
    );
    let (id, path) = (h.show().id.clone(), h.show().path.clone());
    let req = h.app.plugin_view_req();
    let plugin: Arc<dyn konoma_plugin::Plugin> = Arc::new(Mini {
        id: id.clone(),
        group: None,
        seen: h.seen.clone(),
        table: false,
    });
    let open = h.app.plugin_work(
        id.clone(),
        path.clone(),
        plugin_host::PluginJob::Open {
            plugin,
            input: konoma_plugin::Input::File {
                path: path.clone(),
                hint: None,
            },
        },
        req.clone(),
    );
    let known = open.canvas_known.expect("pictures can be shown");
    assert!(known.by_id.is_empty(), "no id of the old session");
    assert!(known.kept.is_empty(), "no source kept for the old session");
    assert!(
        known.front.contains_key(&3),
        "the entry on screen stays (no blink)"
    );
    // Any later job of the session is told, as before.
    let session = h.show_mut().session.take().expect("the session is at hand");
    let view = h
        .app
        .plugin_work(id, path, plugin_host::PluginJob::View { session }, req);
    let known = view.canvas_known.expect("pictures can be shown");
    assert!(known.by_id.contains_key(&3));
    assert!(known.kept.contains_key(&3));
}

/// A plugin that declares a table and a canvas is neither before its first view, and it is a table
/// only while the view it shows is one: its pages do not make it a table.
#[test]
fn a_plugin_of_both_display_types_is_a_table_only_while_it_shows_a_table() {
    let mut h = H::open_as("cs_both", None, false, true);
    assert!(
        matches!(
            h.show().layout.pages,
            Some(konoma_plugin::view::PageLayout::Separate { .. })
        ),
        "it reported pages"
    );
    assert!(h.app.is_canvas_preview(), "it shows a canvas");
    assert!(!h.app.is_table_preview(), "a canvas with pages is no table");
    // Before the first view (loading, or the file read again): neither.
    h.show_mut().view = None;
    assert!(!h.app.is_canvas_preview());
    assert!(
        !h.app.is_table_preview(),
        "pages alone make no table for a plugin that also declares a canvas"
    );
    // A text view instead: still no table.
    h.show_mut().view = Some(View::text(["nothing here"]));
    assert!(!h.app.is_table_preview());
}

/// `plugin_view_now` of a plugin that is not a standard one goes to a worker with the request it
/// was given (not the one the tab would make).
#[test]
fn a_view_asked_for_on_the_spot_from_a_non_standard_plugin_keeps_its_request() {
    let mut h = H::open("cs_view_now", None, true);
    let before = h.seen.reqs.lock().unwrap().len();
    let mut req = ViewReq::new(33, 9);
    req.page = Some(4);
    assert!(h.app.plugin_view_now(req));
    h.settle();
    let reqs = h.seen.reqs.lock().unwrap();
    let got = reqs.get(before).expect("the plugin was asked");
    assert_eq!((got.cols, got.rows, got.page), (33, 9, Some(4)));
}

// ---- round 3 of the mutation audit: queues, user mark, follow-up floor, the request, the frame ------------

use std::time::{Duration, Instant};

#[test]
fn the_queue_bounds_are_the_documented_values() {
    assert_eq!(MAX_QUEUED_ACTS, 256);
    assert_eq!(MAX_QUEUED_KEYS, 4096);
}

/// The show going away takes everything that belonged to it: the user mark, the follow-up floor,
/// the deadline, the keys waiting and the `Esc` latch.
#[test]
fn dropping_the_show_forgets_the_user_mark_the_floor_and_the_deadline() {
    let mut h = H::open("cs_forget", None, false);
    h.app.plugin_user_act = true;
    h.app.canvas_more_at = Some(Instant::now());
    h.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    h.app.plugin_input_left = true;
    h.app.plugin_key_queue.push(waiting_key('a'));
    h.app.drop_plugin();
    assert!(!h.app.plugin_user_act, "the mark of the old show");
    assert!(h.app.canvas_more_at.is_none(), "the floor of the old show");
    assert!(h.app.canvas_redraw_deadline().is_none());
    assert!(!h.app.plugin_input_left);
    assert!(h.app.plugin_key_queue.is_empty());
    assert!(h.app.plugin.is_none());
}

/// The same through the state reset alone (the show is not dropped).
#[test]
fn forgetting_the_canvas_state_clears_the_mark_and_the_floor() {
    let mut h = H::open("cs_forget2", None, false);
    h.app.plugin_user_act = true;
    h.app.canvas_more_at = Some(Instant::now());
    h.app.forget_canvas_state();
    assert!(!h.app.plugin_user_act);
    assert!(h.app.canvas_more_at.is_none());
}

/// The follow-up ask is for a canvas on screen: in the tree it only sets the deadline.
#[test]
fn the_follow_up_ask_is_not_made_for_a_canvas_that_is_not_on_screen() {
    let mut h = H::open("cs_more_hidden", None, false);
    let t0 = Instant::now();
    h.app.clear_plugin_wants_view_for_test();
    h.app.tab.mode = Mode::Tree;
    h.app.canvas_ask_more_for_test(t0);
    assert!(
        !h.app.plugin_wants_view_for_test(),
        "nobody sees the canvas"
    );
    assert_eq!(h.app.canvas_redraw_deadline(), Some(t0));
    assert!(h.app.canvas_more_at.is_none(), "and no ask was made");
}

/// Two deadlines pending: the earlier wins, whichever order they came in.
#[test]
fn the_earliest_of_two_deadlines_wins() {
    let mut h = H::open("cs_more_earliest", None, false);
    h.app.tab.mode = Mode::Tree;
    let t0 = Instant::now() + Duration::from_secs(5);
    // A later deadline is pending; the ask brings an earlier one.
    h.app
        .set_canvas_redraw_at_for_test(Some(t0 + Duration::from_millis(500)));
    h.app.canvas_ask_more_for_test(t0);
    assert_eq!(h.app.canvas_redraw_deadline(), Some(t0));
    // An earlier deadline is pending; the ask brings a later one.
    h.app
        .set_canvas_redraw_at_for_test(Some(t0 - Duration::from_millis(10)));
    h.app.canvas_ask_more_for_test(t0);
    assert_eq!(
        h.app.canvas_redraw_deadline(),
        Some(t0 - Duration::from_millis(10))
    );
}

/// A floor that has passed long ago is not a deadline in the past: the deadline is `now`.
#[test]
fn a_deadline_for_an_ask_that_is_allowed_already_is_now_not_the_past() {
    let mut h = H::open("cs_more_now", None, false);
    h.app.tab.mode = Mode::Tree;
    let t0 = Instant::now();
    h.app.canvas_more_at = Some(t0);
    let now = t0 + Duration::from_millis(100);
    h.app.canvas_ask_more_for_test(now);
    assert_eq!(h.app.canvas_redraw_deadline(), Some(now));
}

/// A timer redraw is an ask like the follow-up: the floor counts from it.
#[test]
fn a_timer_redraw_starts_the_floor_of_the_follow_up_ask() {
    let mut h = H::open("cs_timer_floor", None, false);
    let t0 = Instant::now();
    h.app.set_canvas_redraw_at_for_test(Some(t0));
    let now = t0 + Duration::from_millis(1);
    assert!(h.app.canvas_redraw_if_due(now), "the plugin is asked");
    assert_eq!(h.app.canvas_more_at, Some(now));
    h.app.clear_plugin_wants_view_for_test();
    let soon = now + Duration::from_millis(5);
    h.app.canvas_ask_more_for_test(soon);
    assert!(
        !h.app.plugin_wants_view_for_test(),
        "within the floor of the timer's ask"
    );
    assert_eq!(
        h.app.canvas_redraw_deadline(),
        Some(now + crate::app::MIN_REDRAW_AFTER)
    );
}

/// Each outcome that asks for another answer keeps the user mark for it; nothing else does.
#[test]
fn the_user_mark_is_kept_for_an_answer_that_follows_a_redraw_or_a_reload_only() {
    let mut h = H::open("cs_settle", None, false);
    let outcomes = || -> Vec<(Option<Outcome>, bool)> {
        vec![
            (Some(Outcome::Redraw), true),
            (Some(Outcome::Reload), true),
            (Some(Outcome::Unhandled), false),
            (Some(Outcome::Flash(Tr::new("hello"))), false),
            (None, false),
        ]
    };
    for (outcome, kept) in outcomes() {
        h.app.plugin_user_act = true;
        let desc = format!("{outcome:?}");
        assert!(
            !h.app.settle_user_act(false, outcome),
            "an answer without a view is not the user's yet: {desc}"
        );
        assert_eq!(h.app.plugin_user_act, kept, "{desc}");
    }
    // An answer that carries the view takes the mark, whatever the outcome.
    for (outcome, _) in outcomes() {
        h.app.plugin_user_act = true;
        assert!(h.app.settle_user_act(true, outcome));
        assert!(!h.app.plugin_user_act);
    }
    h.app.plugin_user_act = false;
    assert!(!h.app.settle_user_act(true, Some(Outcome::Redraw)));
}

/// An action that waited in the queue is the user's too when it goes: its answer takes the typing
/// back, and a stale answer can be told apart.
#[test]
fn a_queued_action_that_goes_marks_its_job_as_the_users() {
    let mut h = H::open("cs_queued_mark", Some("cs-queued-mark"), true);
    h.app
        .plugin_groups
        .entry("cs-queued-mark".into())
        .or_default()
        .busy = true;
    assert!(h.app.plugin_act("one"), "queued behind the busy group");
    assert_eq!(h.show().flow.acts.len(), 1);
    h.app.plugin_user_act = false;
    let (gen_before, media_before) = (h.app.plugin_user_act_gen, h.app.media_gen);
    h.app.plugin_groups.get_mut("cs-queued-mark").unwrap().busy = false;
    h.app.flush_plugin_keys();
    assert!(h.app.plugin_user_act, "the job answers the user's action");
    assert_ne!(h.app.media_gen, media_before, "a job was sent");
    assert_eq!(
        h.app.plugin_user_act_gen, h.app.media_gen,
        "the mark belongs to that job"
    );
    assert_ne!(h.app.plugin_user_act_gen, gen_before);
    h.settle();
    assert_eq!(h.seen.acts(), ["one"]);
    assert!(!h.app.plugin_user_act, "and it went with the answer");
}

/// Before the first frame nothing measured the area: 80 columns and one row.
#[test]
fn the_view_request_before_the_first_frame_is_80_by_1_and_follows_the_page() {
    let mut h = H::open("cs_req", None, false);
    h.app.tab.preview_cols = 0;
    h.app.tab.preview_viewport = 0;
    let r = h.app.plugin_view_req();
    assert_eq!((r.cols, r.rows), (80, 1));
    h.app.tab.preview_cols = 33;
    h.app.tab.preview_viewport = 7;
    h.app.tab.page = 3;
    let r = h.app.plugin_view_req();
    assert_eq!((r.cols, r.rows), (33, 7));
    assert_eq!(
        r.page,
        Some(2),
        "pages count from 1 in the tab, from 0 in the request"
    );
    h.app.tab.page = 0;
    assert_eq!(h.app.plugin_view_req().page, Some(0));
}

/// The frame tells the app the size of the area it draws the canvas into, and that it drew.
#[test]
fn drawing_the_canvas_records_the_area_and_that_it_was_drawn() {
    let mut h = H::open("cs_render", None, false);
    h.app.tab.preview_cols = 7;
    h.app.tab.preview_viewport = 7;
    h.app.tab.preview_viewport_drawn = false;
    h.draw();
    // The 90-column screen minus the two border columns; the screen minus the title/tab/footer
    // rows and the two border rows.
    assert_eq!(h.app.tab.preview_cols, 88);
    assert!(
        h.app.tab.preview_viewport > 7 && h.app.tab.preview_viewport < 26,
        "{}",
        h.app.tab.preview_viewport
    );
    assert!(h.app.tab.preview_viewport_drawn);
}

/// A view wanted for later is not wanted any more once it is asked for on the spot.
#[test]
fn a_view_asked_for_when_free_is_not_wanted_again_unless_there_was_no_session() {
    let mut h = H::open("cs_when_free", None, true);
    h.app.clear_plugin_wants_view_for_test();
    h.app.plugin_view_when_free();
    assert!(
        !h.app.plugin_wants_view_for_test(),
        "the request went out at once"
    );
    h.settle();
    let held = h.show_mut().session.take();
    h.app.plugin_view_when_free();
    assert!(
        h.app.plugin_wants_view_for_test(),
        "no session at hand: wanted for when it returns"
    );
    h.show_mut().session = held;
}

/// A wanted view waits for a show that is ready.
#[test]
fn a_wanted_view_waits_until_the_show_is_ready() {
    let mut h = H::open("cs_wanted_ready", None, true);
    h.show_mut().flow.wants_view = true;
    h.show_mut().phase = PluginPhase::Loading;
    let views = h.seen.views.load(Ordering::SeqCst);
    h.app.plugin_session_returned();
    h.settle();
    assert!(h.show().flow.wants_view, "still wanted");
    assert_eq!(h.seen.views.load(Ordering::SeqCst), views, "nobody asked");
    h.show_mut().phase = PluginPhase::Ready;
    h.app.plugin_session_returned();
    h.settle();
    assert!(!h.show().flow.wants_view);
    assert_eq!(h.seen.views.load(Ordering::SeqCst), views + 1);
}

// ---- the run loop's pieces ---------------------------------------------------------------------------

/// What the loop writes to the terminal for the pictures that went, and when it flushes.
#[derive(Default)]
struct Wire {
    written: Vec<u8>,
    flushes: usize,
}

impl std::io::Write for Wire {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn the_deleted_kitty_images_are_written_to_the_terminal_and_flushed() {
    let mut h = H::open("cs_wire", None, false);
    h.app.use_kitty = true;
    h.app.kitty_is_tmux = false;
    h.app.kitty_deletes = vec![5, 6];
    let mut wire = Wire::default();
    crate::flush_kitty_deletes(&mut h.app, &mut wire).unwrap();
    let want = format!(
        "{}{}",
        crate::preview::kitty::build_delete(5, false),
        crate::preview::kitty::build_delete(6, false)
    );
    assert_eq!(String::from_utf8(wire.written.clone()).unwrap(), want);
    assert_eq!(wire.flushes, 1);
    assert!(h.app.kitty_deletes.is_empty(), "taken");
    // Nothing to delete: nothing written, nothing flushed.
    let mut quiet = Wire::default();
    crate::flush_kitty_deletes(&mut h.app, &mut quiet).unwrap();
    assert!(quiet.written.is_empty());
    assert_eq!(quiet.flushes, 0);
    // Not a kitty terminal: the queue is emptied and nothing is written.
    h.app.use_kitty = false;
    h.app.kitty_deletes = vec![9];
    crate::flush_kitty_deletes(&mut h.app, &mut quiet).unwrap();
    assert!(quiet.written.is_empty());
    assert_eq!(quiet.flushes, 0);
    assert!(h.app.kitty_deletes.is_empty());
}

#[test]
fn the_canvas_timer_asks_for_a_new_frame_and_nothing_does_when_it_is_not_due() {
    let mut h = H::open("cs_loop_timer", None, false);
    let t0 = Instant::now();
    assert!(
        !crate::animation_timers_due(&mut h.app, t0),
        "no deadline, no animation"
    );
    h.app
        .set_canvas_redraw_at_for_test(Some(t0 + Duration::from_secs(1)));
    let views = h.seen.views.load(Ordering::SeqCst);
    assert!(!crate::animation_timers_due(&mut h.app, t0), "not yet");
    assert_eq!(h.seen.views.load(Ordering::SeqCst), views);
    assert!(
        crate::animation_timers_due(&mut h.app, t0 + Duration::from_secs(2)),
        "due: the loop draws again"
    );
    assert_eq!(h.seen.views.load(Ordering::SeqCst), views + 1);
}

#[test]
fn only_a_term_that_names_256color_is_the_256_color_palette() {
    use konoma_plugin::session::ColorDepth;
    let d = plugin_host::color_depth_from;
    assert_eq!(d(None, Some("xterm-256color")), ColorDepth::Ansi256);
    assert_eq!(d(None, Some("screen-256color")), ColorDepth::Ansi256);
    // `256` alone does not say "256color".
    assert_eq!(d(None, Some("xterm-256")), ColorDepth::Ansi16);
    assert_eq!(d(None, Some("xterm")), ColorDepth::Ansi16);
}

/// The canvas frame itself records the size of the area it draws into (the preview frame does it
/// too before handing over; the canvas must not depend on that).
#[test]
fn the_canvas_frame_records_its_own_area() {
    let mut h = H::open("cs_render_direct", None, false);
    h.app.tab.preview_cols = 7;
    h.app.tab.preview_viewport = 7;
    h.app.tab.preview_viewport_drawn = false;
    let app = &mut h.app;
    h.term
        .draw(|f| {
            crate::ui::canvas::render(f, app, ratatui::layout::Rect::new(0, 0, 30, 10));
        })
        .expect("draw");
    assert_eq!(h.app.tab.preview_cols, 28);
    assert_eq!(h.app.tab.preview_viewport, 8);
    assert!(h.app.tab.preview_viewport_drawn);
}
