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
}

impl Plugin for Mini {
    fn manifest(&self) -> Manifest {
        let m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.mini"]))
            .shows(ViewKind::Canvas);
        match &self.group {
            Some(g) => m.work_group(g.as_str()),
            None => m,
        }
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(MiniSession {
            seen: self.seen.clone(),
        }))
    }
}

struct MiniSession {
    seen: Arc<Seen>,
}

impl Session for MiniSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.seen.views.fetch_add(1, Ordering::SeqCst);
        let mut c = CanvasView::new(req.cols, req.rows);
        c.set_string(0, 0, "mini", konoma_plugin::Style::new());
        c.text_input = true;
        Ok(View::Canvas(c))
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        self.seen.keys.lock().unwrap().push(key.clone());
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
