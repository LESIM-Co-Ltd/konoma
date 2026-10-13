//! The host side of the plugin API: opening a plugin, keeping its `Session`, and handing the
//! display type it returns to the host's own widgets.
//!
//! The plugin's work (`open` with its first `view`, later `view`s and `act`s) runs on a worker
//! thread; this module owns the one **plugin show** the app has (`App::plugin`, like the
//! workbook and the Word document it sits beside: heavy data lives on `App`, a tab switch opens it
//! again). The show is the plugin id, the `Session`, its `Layout`, the current `View` and the
//! request that produced it.
//!
//! How a job runs:
//! - The job is a [`PluginWork`]. With a channel attached (`attach_plugin_runner`) it goes to a
//!   thread of its own and the result comes back as a [`PluginResult`] through the one plugin
//!   channel; without one (tests) it runs on the spot and is applied at once.
//! - It is cancelled the way every media job is: `bump_media_gen` makes the result stale (dropped on
//!   arrival) and the running plugin sees its `Cancel` turn true (the shared generation moved).
//! - The first frame: after starting an open the UI waits up to [`FIRST_FRAME_BUDGET`] for the
//!   result, so a small file shows exactly as it always did; if the plugin is slower the frame
//!   shows "Loading..." and the result is applied when it lands. Input is not lost while
//!   waiting: the terminal buffers it.
//! - A panic in the plugin is caught on the worker and reported as a failure, so the loading mark
//!   can never stick.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use konoma_plugin::view::{Layout, TableView, View, ViewReq};
use konoma_plugin::{
    t, ActCx, Cancel, Failure, Input, OpenCx, Outcome, Plugin, Session, Settings, TermCaps, ViewCx,
};

use super::*;

/// How long the UI waits for a freshly started `open` before it draws "Loading...".
pub(crate) const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(30);

/// Where the plugin show stands.
pub(crate) enum PluginPhase {
    /// The first `open` has not come back.
    Loading,
    /// The view is there.
    Ready,
    /// The plugin could not open or draw the file; the host shows the old kind's fallback.
    Failed(Failure),
}

/// The plugin display the app has now.
pub(crate) struct PluginShow {
    /// The plugin id.
    pub id: String,
    /// The file it shows.
    pub path: PathBuf,
    pub phase: PluginPhase,
    /// `None` while a job holds it on the worker (or when the plugin failed).
    pub session: Option<Box<dyn Session>>,
    /// What the session reported about its representations and pages (read by the plugins wired
    /// in the next steps).
    #[allow(dead_code)]
    pub layout: Layout,
    /// The latest view. It stays on screen while the same file is read again.
    pub view: Option<View>,
    /// The request the view answers.
    pub req: ViewReq,
}

/// What a worker is asked to do.
pub(crate) enum PluginJob {
    /// `open`, then the first `view`.
    Open {
        plugin: Arc<dyn Plugin>,
        input: Input,
    },
    /// Another `view` of an open session.
    View { session: Box<dyn Session> },
    /// A declared action of an open session.
    Act {
        session: Box<dyn Session>,
        action: String,
    },
}

/// One job with everything it needs, owned (it moves to a thread).
pub(crate) struct PluginWork {
    id: String,
    gen: u64,
    job: PluginJob,
    req: ViewReq,
    settings: Settings,
    lang: String,
    pixels: bool,
}

/// What a worker reports back. Sent through the one plugin channel.
pub struct PluginResult {
    gen: u64,
    id: String,
    /// The session, handed back (`None` when the plugin failed to open or panicked).
    session: Option<Box<dyn Session>>,
    layout: Option<Layout>,
    view: Option<View>,
    outcome: Option<Outcome>,
    failure: Option<Failure>,
    /// The request a `view` answered.
    req: ViewReq,
}

impl PluginResult {
    pub(super) fn failed(gen: u64, id: String, req: ViewReq, failure: Failure) -> PluginResult {
        PluginResult {
            gen,
            id,
            session: None,
            layout: None,
            view: None,
            outcome: None,
            failure: Some(failure),
            req,
        }
    }
}

impl PluginWork {
    /// Run the job. Always returns a result, even when the plugin panics.
    fn run(self, cancel: Cancel) -> PluginResult {
        let (id, gen, req) = (self.id.clone(), self.gen, self.req.clone());
        konoma_base::panic::catch_silent(move || self.run_inner(cancel)).unwrap_or_else(|| {
            PluginResult::failed(
                gen,
                id,
                req,
                Failure::external(t!(
                    en: "the plugin stopped unexpectedly",
                    ja: "プラグインが予期せず停止しました",
                )),
            )
        })
    }

    fn run_inner(self, cancel: Cancel) -> PluginResult {
        let PluginWork {
            id,
            gen,
            job,
            req,
            settings,
            lang,
            pixels,
        } = self;
        let term = TermCaps { pixels };
        let view_of = |mut session: Box<dyn Session>, req: ViewReq| {
            let cx = ViewCx::new(&settings, &lang)
                .with_cancel(cancel.clone())
                .with_term(term);
            let layout = session.layout();
            let (view, failure) = match session.view(&req, &cx) {
                Ok(v) => (Some(v), None),
                Err(f) => (None, Some(f)),
            };
            PluginResult {
                gen,
                id: id.clone(),
                session: Some(session),
                layout: Some(layout),
                view,
                outcome: None,
                failure,
                req,
            }
        };
        match job {
            PluginJob::Open { plugin, input } => {
                let cx = OpenCx::new(&settings, &lang)
                    .with_cancel(cancel.clone())
                    .with_term(term);
                match plugin.open(input, &cx) {
                    Ok(session) => view_of(session, req),
                    Err(f) => PluginResult::failed(gen, id.clone(), req, f),
                }
            }
            PluginJob::View { session } => view_of(session, req),
            PluginJob::Act {
                mut session,
                action,
            } => {
                let cx = ActCx::new(&settings, &lang)
                    .with_cancel(cancel.clone())
                    .with_term(term);
                // A failed action is told to the user and leaves the view as it was.
                let (outcome, failure) = match session.act(&action, &cx) {
                    Ok(o) => (Some(o), None),
                    Err(f) => (Some(Outcome::Flash(f.message.clone())), None),
                };
                PluginResult {
                    gen,
                    id,
                    layout: Some(session.layout()),
                    session: Some(session),
                    view: None,
                    outcome,
                    failure,
                    req,
                }
            }
        }
    }
}

/// The meeting point of the UI thread (waiting for the first frame) and a worker. The worker
/// leaves its result here if the UI is still waiting, and sends it through the channel when the UI
/// has given up, so exactly one side gets it.
pub(super) struct Handoff {
    slot: Mutex<Slot>,
    ready: Condvar,
}

enum Slot {
    Waiting,
    Done(Box<PluginResult>),
    GaveUp,
}

impl Handoff {
    pub(super) fn new() -> Handoff {
        Handoff {
            slot: Mutex::new(Slot::Waiting),
            ready: Condvar::new(),
        }
    }

    /// Worker side.
    pub(super) fn deliver(&self, result: PluginResult, tx: &std::sync::mpsc::Sender<PluginResult>) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Waiting) {
            *slot = Slot::Done(Box::new(result));
            self.ready.notify_all();
            return;
        }
        drop(slot);
        let _ = tx.send(result);
    }

    /// UI side: the result if it came within `budget`; otherwise the UI gives up on waiting and the
    /// worker will send through the channel.
    pub(super) fn wait(&self, budget: Duration) -> Option<PluginResult> {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let (mut slot, _) = self
            .ready
            .wait_timeout_while(slot, budget, |s| matches!(s, Slot::Waiting))
            .unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *slot, Slot::GaveUp) {
            Slot::Done(r) => Some(*r),
            _ => None,
        }
    }
}

impl App {
    /// Attach the sending end that carries plugin results back to the run loop. Without it (tests)
    /// plugin jobs run on the spot.
    pub fn attach_plugin_runner(&mut self, tx: std::sync::mpsc::Sender<PluginResult>) {
        self.plugin_tx = Some(tx);
    }

    /// Whether the preview is a plugin's and its first result has not landed: the "Loading..."
    /// screen. (A file read *again* keeps its old view on screen instead.)
    pub fn is_plugin_loading(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Plugin { .. }))
            && self
                .plugin
                .as_ref()
                .is_some_and(|p| matches!(p.phase, PluginPhase::Loading))
    }

    /// The table the plugin returned, if the show has one on screen.
    pub(super) fn plugin_table(&self) -> Option<&TableView> {
        match self.plugin.as_ref()?.view.as_ref()? {
            View::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Why the plugin could not show the file (`None` while loading or when it worked).
    pub fn plugin_failure(&self) -> Option<&Failure> {
        match &self.plugin.as_ref()?.phase {
            PluginPhase::Failed(f) => Some(f),
            _ => None,
        }
    }

    /// Test-only: the layout the open session reported.
    #[cfg(test)]
    pub(crate) fn plugin_layout_for_test(&self) -> Option<&Layout> {
        self.plugin.as_ref().map(|p| &p.layout)
    }

    /// Release the plugin show (off the UI thread: a session can hold a large table).
    pub(super) fn drop_plugin(&mut self) {
        if let Some(old) = self.plugin.take() {
            super::table_actions::discard_in_background(old);
        }
    }

    /// Open the plugin that shows the current preview (`PreviewKind::Plugin`) afresh: whatever was
    /// shown is dropped and the screen is "Loading..." until the result lands. For any other kind
    /// the show is simply dropped.
    pub(super) fn load_plugin(&mut self) {
        self.open_plugin(false);
    }

    /// Read the plugin's file again (the file changed): the old view stays on screen until the new
    /// one lands.
    pub(super) fn reload_plugin(&mut self) {
        self.open_plugin(true);
    }

    /// Test-friendly name of [`App::load_plugin`] (the table tests call it by the old name).
    #[cfg(test)]
    pub(super) fn load_table(&mut self) {
        self.load_plugin();
    }

    fn open_plugin(&mut self, keep_previous: bool) {
        let Some(PreviewKind::Plugin { path, id, hint }) = self.tab.preview_kind.clone() else {
            self.drop_plugin();
            return;
        };
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let plugin = if registry.is_enabled(&id) {
            registry.plugin(&id)
        } else {
            None
        };
        let Some(plugin) = plugin else {
            // Switched off or unknown after the file was decided: the same screen as a failure.
            self.drop_plugin();
            self.plugin = Some(PluginShow {
                id,
                path,
                phase: PluginPhase::Failed(Failure::unsupported(t!(
                    en: "the plugin is not available",
                    ja: "プラグインを使えません",
                ))),
                session: None,
                layout: Layout::single(),
                view: None,
                req: ViewReq::default(),
            });
            return;
        };
        let input = Input::File { path, hint };
        self.start_plugin_open(id, plugin, input, keep_previous);
    }

    /// The request a view is asked with: the size of the preview area as far as it is known.
    fn plugin_view_req(&self) -> ViewReq {
        let rows = self.tab.preview_viewport.max(1);
        ViewReq::new(80, rows)
    }

    /// Start opening `plugin` on `input` for the current preview and, within the first-frame
    /// budget, apply the result if it comes.
    pub(super) fn start_plugin_open(
        &mut self,
        id: String,
        plugin: Arc<dyn Plugin>,
        input: Input,
        keep_previous: bool,
    ) {
        let path = match &input {
            Input::File { path, .. } => path.clone(),
            _ => self.tab.preview_path.clone().unwrap_or_default(),
        };
        let same = self
            .plugin
            .as_ref()
            .is_some_and(|p| p.id == id && p.path == path && p.view.is_some());
        if !(keep_previous && same) {
            self.drop_plugin();
            self.plugin = Some(PluginShow {
                id: id.clone(),
                path,
                phase: PluginPhase::Loading,
                session: None,
                layout: Layout::single(),
                view: None,
                req: ViewReq::default(),
            });
        }
        let work = PluginWork {
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            pixels: self.terminal_draws_pixels(),
            req: self.plugin_view_req(),
            id,
            gen: 0,
            job: PluginJob::Open { plugin, input },
        };
        self.dispatch_plugin_work(work, true);
    }

    /// The user's language as the plugins name it.
    fn plugin_lang(&self) -> &'static str {
        match self.lang {
            crate::i18n::Lang::Jp => "ja",
            crate::i18n::Lang::En => "en",
        }
    }

    /// Give `work` a new generation (everything older is stale), run it, and either apply its
    /// result on the spot (no channel; or it came within the first-frame budget when `first_frame`)
    /// or leave it to the run loop.
    fn dispatch_plugin_work(&mut self, mut work: PluginWork, first_frame: bool) {
        self.bump_media_gen();
        work.gen = self.media_gen;
        let (shared, gen) = (self.media_gen_shared.clone(), work.gen);
        let cancel =
            Cancel::from_fn(move || shared.load(std::sync::atomic::Ordering::Relaxed) != gen);
        let Some(tx) = self.plugin_tx.clone() else {
            // No worker attached (tests): run here, apply now.
            self.media_loading = true;
            let result = work.run(cancel);
            self.install_plugin_result(result);
            return;
        };
        self.media_loading = true;
        let handoff = Arc::new(Handoff::new());
        let worker_handoff = Arc::clone(&handoff);
        let work = Arc::new(Mutex::new(Some(work)));
        let worker_work = Arc::clone(&work);
        let worker_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("konoma-plugin".into())
            .spawn(move || {
                let taken = worker_work.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(work) = taken {
                    let result = work.run(worker_cancel);
                    worker_handoff.deliver(result, &tx);
                }
            });
        if spawned.is_err() {
            // No thread to be had: do the work here, as the tests do.
            let taken = work.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(work) = taken {
                let result = work.run(cancel);
                self.install_plugin_result(result);
            }
            return;
        }
        // The first frame waits for a fresh open; a later `view` or `act` does not wait (the old
        // view stays on screen), but still takes a result that is already there.
        let budget = if first_frame {
            FIRST_FRAME_BUDGET
        } else {
            Duration::ZERO
        };
        if let Some(result) = handoff.wait(budget) {
            self.install_plugin_result(result);
        }
    }

    /// Ask the open session for another `view` (a new page, a new size...). Returns false when
    /// there is no session to ask (none open, or a job still holds it).
    #[allow(dead_code)] // The plugins wired in the next steps (pages, representations) ask this way.
    pub(super) fn plugin_request_view(&mut self, mut req: ViewReq) -> bool {
        let Some(show) = self.plugin.as_mut() else {
            return false;
        };
        let Some(session) = show.session.take() else {
            return false;
        };
        let id = show.id.clone();
        if req.cols == 0 || req.rows == 0 {
            req = self.plugin_view_req();
        }
        let work = PluginWork {
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            pixels: self.terminal_draws_pixels(),
            req,
            id,
            gen: 0,
            job: PluginJob::View { session },
        };
        self.dispatch_plugin_work(work, false);
        true
    }

    /// Run a declared action of the open session. Returns false when there is no session to ask.
    #[allow(dead_code)] // The keys of the plugins wired in the next steps run their actions this way.
    pub(super) fn plugin_act(&mut self, action: &str) -> bool {
        let Some(show) = self.plugin.as_mut() else {
            return false;
        };
        let Some(session) = show.session.take() else {
            return false;
        };
        let (id, req) = (show.id.clone(), show.req.clone());
        let work = PluginWork {
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            pixels: self.terminal_draws_pixels(),
            req,
            id,
            gen: 0,
            job: PluginJob::Act {
                session,
                action: action.to_string(),
            },
        };
        self.dispatch_plugin_work(work, false);
        true
    }

    /// Put a result into the show, if it still belongs there. Returns whether the screen changed.
    /// Does not touch the cursor or the search: the callers that run inside a larger transition
    /// (`enter_preview`, a tab restore) are not in a state where those could be judged.
    fn install_plugin_result(&mut self, res: PluginResult) -> bool {
        // The generation is the newest: no other job is in flight under it, so whatever happens to
        // this result the loading mark ends with it.
        let current = res.gen == self.media_gen;
        if current {
            self.media_loading = false;
        }
        // Stale (the preview moved on): drop it, with its session, off the UI thread.
        let belongs = self
            .plugin
            .as_ref()
            .is_some_and(|p| p.id == res.id && self.shows_plugin_file(p));
        if !current || !belongs {
            super::table_actions::discard_in_background(res);
            return false;
        }
        let outcome = res.outcome.clone();
        let Some(show) = self.plugin.as_mut() else {
            return false;
        };
        show.session = res.session;
        if let Some(layout) = res.layout {
            show.layout = layout;
        }
        match (res.view, res.failure) {
            (Some(view), _) => {
                show.view = Some(view);
                show.req = res.req;
                show.phase = PluginPhase::Ready;
            }
            (None, Some(failure)) => {
                show.view = None;
                show.phase = PluginPhase::Failed(failure);
            }
            // An action's result: the view is what it was.
            (None, None) => {}
        }
        match outcome {
            Some(Outcome::Flash(msg)) => {
                self.flash = Some(msg.get(self.plugin_lang()).to_string());
            }
            Some(Outcome::Reload) => self.reload_plugin(),
            Some(Outcome::Redraw) | Some(Outcome::Unhandled) | None => {}
        }
        true
    }

    /// Whether the preview on screen is the file `show` was opened for.
    fn shows_plugin_file(&self, show: &PluginShow) -> bool {
        matches!(
            &self.tab.preview_kind,
            Some(PreviewKind::Plugin { id, path, .. }) if *id == show.id && *path == show.path
        )
    }

    /// Apply a result that came through the plugin channel (the run loop). Stale results are
    /// dropped. Returns whether the screen changed.
    pub fn apply_plugin_result(&mut self, res: PluginResult) -> bool {
        if !self.install_plugin_result(res) {
            return false;
        }
        // The file landed after the transition that asked for it: the cursor the tab saved may be
        // past the end of what was read, and a search in force runs on the new cells.
        self.clamp_table_cursor();
        self.rescan_table_search();
        true
    }
}
