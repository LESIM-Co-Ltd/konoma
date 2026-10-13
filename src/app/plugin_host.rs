//! The host side of the plugin API: opening a plugin, keeping its `Session`, and handing the
//! display type it returns to the host's own widgets. **Every plugin goes through this one road**:
//! the tables (`csv`, `archive`) and the pictures (`image`, `svg`, `pdf`, `video`) alike.
//!
//! The plugin's work (`open` with its first `view`, later `view`s and `act`s) runs on a worker
//! thread; this module owns the one **plugin show** the app has (`App::plugin`, like the
//! workbook and the Word document it sits beside: heavy data lives on `App`, a tab switch opens it
//! again). The show is the plugin id, the `Session`, its `Layout`, the current `View` and the
//! request that produced it.
//!
//! How a job runs:
//! - The job is a [`PluginWork`]. With the one worker channel attached (`attach_media_loader`) it
//!   goes to a thread of its own and the result comes back as a [`PluginResult`] inside a
//!   [`MediaResult`] under the one generation (`media_gen`); without a channel (tests) it runs on
//!   the spot and is applied at once.
//! - When the plugin answers with a picture, the worker decodes it **in the same job** and the
//!   result carries the decoded [`MediaPayload`] (no second trip through a worker).
//! - It is cancelled the way every media job is: `bump_media_gen` makes the result stale (dropped on
//!   arrival) and the running plugin sees its `Cancel` turn true (the shared generation moved).
//! - The first frame, for a table: after starting an open the UI waits up to
//!   [`FIRST_FRAME_BUDGET`] for the result, so a small file shows exactly as it always did; if the
//!   plugin is slower the frame shows "Loading..." and the result is applied when it lands. Input
//!   is not lost while waiting: the terminal buffers it.
//! - The first frame, for a picture: the UI waits up to the same budget only for the **layout**
//!   (what `open` learned: the page count of a PDF), which the worker hands over as soon as `open`
//!   returns, before the first page is drawn. The picture itself always arrives through the
//!   channel and the old "loading" look (spinner, the old page kept on screen) is unchanged.
//! - A panic in the plugin is caught on the worker and reported as a failure, so the loading mark
//!   can never stick.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use konoma_plugin::view::{Layout, PageLayout, TableView, View, ViewReq};
use konoma_plugin::{
    t, ActCx, Cancel, Failure, Input, OpenCx, Outcome, Plugin, Session, Settings, TermCaps, ViewCx,
};

use super::media_plugin::image_failure_of;
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
    /// What the session reported about its representations and pages. The page count is the
    /// host's (`PerTab::page_count`); the rest is read by the plugins wired in the next steps.
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

/// What a worker calls with the layout `open` learned.
pub(super) type LayoutAnnounce = Box<dyn Fn(&Layout) + Send>;

/// One job with everything it needs, owned (it moves to a thread).
pub(crate) struct PluginWork {
    pub(super) id: String,
    pub(super) gen: u64,
    pub(super) job: PluginJob,
    pub(super) req: ViewReq,
    pub(super) settings: Settings,
    pub(super) lang: String,
    pub(super) pixels: bool,
    /// The file shown (a picture's relative references resolve from it).
    pub(super) path: PathBuf,
    /// Whether the plugin may start an external program (`[external] pdf` / `video`).
    pub(super) external: bool,
    /// The longest side to rasterize a vector source at when the plugin does not say.
    pub(super) default_raster_px: u32,
    /// Whether the plugin is one of the picture plugins: its `view` must be a picture, and the
    /// picture is decoded here, on the worker.
    pub(super) expects_picture: bool,
    /// What stops the drawing of an untrusted SVG (the generation moved on).
    pub(super) decode_cancel: Option<crate::preview::office::Cancel>,
    /// Called with the layout as soon as `open` returns, before the first `view`.
    pub(super) on_layout: Option<LayoutAnnounce>,
}

/// What became of a picture a worker decoded.
pub(super) enum Picture {
    /// The view was not a picture.
    None,
    /// The decoded picture (or the reason it could not be loaded, as `MediaPayload::ImageFailed`).
    Decoded(Box<MediaPayload>),
    /// The view was a picture that could not be made into pixels, with no reason to show.
    Failed,
}

/// What a worker reports back. Sent through the one worker channel, inside a [`MediaResult`].
pub struct PluginResult {
    pub(super) gen: u64,
    pub(super) id: String,
    /// The session, handed back (`None` when the plugin failed to open or panicked).
    pub(super) session: Option<Box<dyn Session>>,
    pub(super) layout: Option<Layout>,
    pub(super) view: Option<View>,
    pub(super) outcome: Option<Outcome>,
    pub(super) failure: Option<Failure>,
    /// The request a `view` answered.
    pub(super) req: ViewReq,
    /// The picture the view held, decoded on the worker.
    pub(super) picture: Picture,
}

/// The layout a worker learned from `open`, sent ahead of the (slower) first picture when the UI
/// was no longer waiting for it.
pub struct PluginLayoutMsg {
    id: String,
    layout: Layout,
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
            picture: Picture::None,
        }
    }
}

impl PluginResult {
    /// Test-only: the picture this result stands for: the decoded picture, or the reason it could
    /// not be made (a failed read of an SVG file is an `ImageFailed` like a failed decode).
    #[cfg(test)]
    pub(super) fn into_image_payload(self) -> Option<MediaPayload> {
        match self.picture {
            Picture::Decoded(p) => Some(*p),
            Picture::Failed => None,
            Picture::None => self
                .failure
                .and_then(|f| super::media_plugin::image_failure_of(&self.id, &f))
                .map(MediaPayload::ImageFailed),
        }
    }
}

impl PluginWork {
    /// Test-only: run the job here, with nothing to cancel it.
    #[cfg(test)]
    pub(super) fn run_on_the_spot(self) -> PluginResult {
        self.run(Cancel::from_fn(|| false))
    }

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
            path,
            external,
            default_raster_px,
            expects_picture,
            decode_cancel,
            on_layout,
        } = self;
        let term = TermCaps { pixels };
        let view_of = |mut session: Box<dyn Session>, req: ViewReq| {
            let cx = ViewCx::new(&settings, &lang)
                .with_cancel(cancel.clone())
                .with_term(term)
                .with_external(external);
            let layout = session.layout();
            let mut picture = Picture::None;
            let (view, failure) = match session.view(&req, &cx) {
                Ok(View::Image(iv)) => {
                    picture = match super::media_plugin::decode_image_view(
                        iv,
                        &path,
                        default_raster_px,
                        decode_cancel.as_ref(),
                    ) {
                        Some(p) => Picture::Decoded(Box::new(p)),
                        None => Picture::Failed,
                    };
                    (None, None)
                }
                Ok(_) if expects_picture => (
                    None,
                    Some(Failure::unsupported(t!(
                        en: "the plugin did not return a picture",
                        ja: "プラグインが画像を返しませんでした",
                    ))),
                ),
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
                picture,
            }
        };
        match job {
            PluginJob::Open { plugin, input } => {
                let cx = OpenCx::new(&settings, &lang)
                    .with_cancel(cancel.clone())
                    .with_term(term)
                    .with_external(external);
                match plugin.open(input, &cx) {
                    Ok(session) => {
                        // What `open` learned (a PDF's page count) goes to the host now, before the
                        // first page is drawn.
                        if let Some(announce) = &on_layout {
                            announce(&session.layout());
                        }
                        view_of(session, req)
                    }
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
                    picture: Picture::None,
                }
            }
        }
    }
}

/// Wrap a plugin result for the one worker channel.
fn media_result(result: PluginResult) -> MediaResult {
    MediaResult {
        gen: result.gen,
        wb_worker: false,
        payload: Some(MediaPayload::Plugin(Box::new(result))),
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
    pub(super) fn deliver(&self, result: PluginResult, tx: &std::sync::mpsc::Sender<MediaResult>) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Waiting) {
            *slot = Slot::Done(Box::new(result));
            self.ready.notify_all();
            return;
        }
        drop(slot);
        let _ = tx.send(media_result(result));
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

/// The meeting point for the **layout** of a picture plugin: the UI waits a short while for what
/// `open` learned (so the first frame already knows the page count); the picture itself is never
/// handed over here. If the UI gave up first, the worker sends the layout through the channel.
pub(super) struct LayoutGate {
    slot: Mutex<Gate>,
    ready: Condvar,
}

enum Gate {
    Waiting,
    /// `open` returned: its layout, or `None` when it failed.
    Ready(Option<Layout>),
    GaveUp,
}

impl LayoutGate {
    pub(super) fn new() -> LayoutGate {
        LayoutGate {
            slot: Mutex::new(Gate::Waiting),
            ready: Condvar::new(),
        }
    }

    /// Worker side. Returns the layout back when the UI is no longer waiting, for the caller to
    /// send through the channel.
    pub(super) fn publish(&self, layout: Layout) -> Option<Layout> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match *slot {
            Gate::Waiting => {
                *slot = Gate::Ready(Some(layout));
                self.ready.notify_all();
                None
            }
            Gate::GaveUp => Some(layout),
            Gate::Ready(_) => None,
        }
    }

    /// Worker side: the job is over; a UI still waiting stops waiting.
    pub(super) fn release(&self) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Gate::Waiting) {
            *slot = Gate::Ready(None);
            self.ready.notify_all();
        }
    }

    /// UI side: the layout if `open` returned within `budget`.
    pub(super) fn wait(&self, budget: Duration) -> Option<Layout> {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let (mut slot, _) = self
            .ready
            .wait_timeout_while(slot, budget, |s| matches!(s, Gate::Waiting))
            .unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *slot, Gate::GaveUp) {
            Gate::Ready(layout) => layout,
            _ => None,
        }
    }
}

/// What the UI thread waits for after starting a job.
pub(super) enum Wait {
    /// Nothing: the result comes through the channel.
    Nothing,
    /// The whole result, for up to this long (a table's first frame). A zero wait still takes a
    /// result that is already there.
    Result(Duration),
    /// Only the layout `open` learned, for up to this long (a picture's first frame).
    Layout(Duration),
}

/// Whether the page layout of a session is one the host's page keys turn.
fn page_count_of(layout: &Layout) -> Option<Option<u32>> {
    match &layout.pages {
        Some(PageLayout::Separate { count, .. }) => Some(count.and_then(|n| u32::try_from(n).ok())),
        _ => None,
    }
}

impl App {
    /// Whether the preview is a plugin's and its first result has not landed: the "Loading..."
    /// screen. (A file read *again* keeps its old view on screen instead; and a picture's loading
    /// look is the image pipeline's own spinner, not this screen.)
    pub fn is_plugin_loading(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Plugin { .. }))
            && !self.shows_media_plugin()
            // Text is read by the host in windows, so the file is on screen (plain) before the
            // plugin has answered; there is no "Loading..." screen for it.
            && !self.shows_code_plugin()
            && self
                .plugin
                .as_ref()
                .is_some_and(|p| matches!(p.phase, PluginPhase::Loading))
    }

    /// Whether the show holds a text view.
    fn plugin_view_is_text(&self) -> bool {
        matches!(
            self.plugin.as_ref().and_then(|p| p.view.as_ref()),
            Some(View::Text(_))
        )
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
        // The picture plugins (a mermaid file among them) are started with the media load
        // (`start_media_load`), which comes first and owns their show; this is not the place to
        // touch it.
        if self
            .tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_media_plugin)
        {
            return;
        }
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
    pub(super) fn plugin_view_req(&self) -> ViewReq {
        let rows = self.tab.preview_viewport.max(1);
        ViewReq::new(80, rows)
    }

    /// A job for the plugin `id` on the file `path`, with everything the worker needs from the
    /// app's state. The generation is given when it is dispatched.
    pub(super) fn plugin_work(
        &self,
        id: String,
        path: PathBuf,
        job: PluginJob,
        req: ViewReq,
    ) -> PluginWork {
        let external = match id.as_str() {
            konoma_media::PDF_ID => self.cfg.external.pdf,
            konoma_media::VIDEO_ID => self.cfg.external.video,
            _ => false,
        };
        PluginWork {
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            pixels: self.terminal_draws_pixels(),
            expects_picture: crate::preview::PreviewKind::is_media_plugin_id(&id),
            id,
            path,
            external,
            default_raster_px: self.cfg.ui.svg_max_px,
            decode_cancel: None,
            on_layout: None,
            gen: 0,
            job,
            req,
        }
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
                path: path.clone(),
                phase: PluginPhase::Loading,
                session: None,
                layout: Layout::single(),
                view: None,
                req: ViewReq::default(),
            });
        }
        let work = self.plugin_work(
            id,
            path,
            PluginJob::Open { plugin, input },
            self.plugin_view_req(),
        );
        self.dispatch_plugin_work(work, Wait::Result(FIRST_FRAME_BUDGET));
    }

    /// The user's language as the plugins name it.
    pub(super) fn plugin_lang(&self) -> &'static str {
        match self.lang {
            crate::i18n::Lang::Jp => "ja",
            crate::i18n::Lang::En => "en",
        }
    }

    /// Give `work` a new generation (everything older is stale), run it, and either apply its
    /// result on the spot (no channel; or it came within the first-frame budget of a
    /// `Wait::Result`) or leave it to the run loop.
    pub(super) fn dispatch_plugin_work(&mut self, mut work: PluginWork, wait: Wait) {
        self.bump_media_gen();
        work.gen = self.media_gen;
        let (shared, gen) = (self.media_gen_shared.clone(), work.gen);
        let cancel = {
            let shared = shared.clone();
            Cancel::from_fn(move || shared.load(std::sync::atomic::Ordering::Relaxed) != gen)
        };
        work.decode_cancel = Some(crate::preview::office::Cancel::generation(
            shared.clone(),
            gen,
        ));
        self.media_loading = true;
        let Some(tx) = self.media_tx.clone() else {
            // No worker attached (tests): run here, apply now.
            let result = work.run(cancel);
            self.install_plugin_result(result);
            return;
        };
        let handoff = matches!(wait, Wait::Result(_)).then(|| Arc::new(Handoff::new()));
        let gate = matches!(wait, Wait::Layout(_)).then(|| Arc::new(LayoutGate::new()));
        if let Some(gate) = &gate {
            let (gate, tx, id) = (Arc::clone(gate), tx.clone(), work.id.clone());
            work.on_layout = Some(Box::new(move |layout: &Layout| {
                // The UI stopped waiting: the layout goes ahead of the picture through the channel.
                if let Some(layout) = gate.publish(layout.clone()) {
                    let _ = tx.send(MediaResult {
                        gen,
                        wb_worker: false,
                        payload: Some(MediaPayload::PluginLayout(Box::new(PluginLayoutMsg {
                            id: id.clone(),
                            layout,
                        }))),
                    });
                }
            }));
        }
        // The decode queues in the shared decode gate as the thing on screen (top priority), and
        // gives up if the preview moves on before its turn.
        let ticket = Arc::new(crate::preview::image::DecodeTicket::for_generation(
            u64::MAX,
            shared,
            gen,
        ));
        let work = Arc::new(Mutex::new(Some(work)));
        let worker = (
            Arc::clone(&work),
            handoff.clone(),
            gate.clone(),
            tx,
            cancel.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("konoma-plugin".into())
            .spawn(move || {
                let (work, handoff, gate, tx, cancel) = worker;
                let taken = work.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(work) = taken {
                    let result =
                        crate::preview::image::with_decode_ticket(ticket, || work.run(cancel));
                    match &handoff {
                        Some(h) => h.deliver(result, &tx),
                        None => {
                            let _ = tx.send(media_result(result));
                        }
                    }
                }
                // A UI still waiting for the layout of a job that never got that far (a failed
                // open) stops waiting.
                if let Some(gate) = &gate {
                    gate.release();
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
        match wait {
            Wait::Nothing => {}
            Wait::Result(budget) => {
                if let Some(result) = handoff.and_then(|h| h.wait(budget)) {
                    self.install_plugin_result(result);
                }
            }
            Wait::Layout(budget) => {
                if let Some(layout) = gate.and_then(|g| g.wait(budget)) {
                    self.adopt_plugin_layout(&layout);
                }
            }
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
        let (id, path) = (show.id.clone(), show.path.clone());
        if req.cols == 0 || req.rows == 0 {
            req = self.plugin_view_req();
        }
        let work = self.plugin_work(id, path, PluginJob::View { session }, req);
        self.dispatch_plugin_work(work, Wait::Result(Duration::ZERO));
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
        let (id, path, req) = (show.id.clone(), show.path.clone(), show.req.clone());
        let work = self.plugin_work(
            id,
            path,
            PluginJob::Act {
                session,
                action: action.to_string(),
            },
            req,
        );
        self.dispatch_plugin_work(work, Wait::Result(Duration::ZERO));
        true
    }

    /// Take what a session reported about its pages as the host's page state
    /// (`PerTab::page_count`, and the page clamped to it).
    fn adopt_layout(&mut self, layout: &Layout) {
        if let Some(count) = page_count_of(layout) {
            self.tab.page_count = count;
            if let Some(n) = count {
                self.tab.page = self.tab.page.clamp(1, n.max(1));
            }
        }
    }

    /// The layout `open` reported reached the host before the first picture did.
    fn adopt_plugin_layout(&mut self, layout: &Layout) {
        if let Some(show) = self.plugin.as_mut() {
            show.layout = layout.clone();
        }
        self.adopt_layout(layout);
    }

    /// A layout sent ahead of the picture through the channel (current generation only).
    pub(super) fn apply_plugin_layout(&mut self, msg: PluginLayoutMsg) -> bool {
        let belongs = self
            .plugin
            .as_ref()
            .is_some_and(|p| p.id == msg.id && self.shows_plugin_file(p));
        if !belongs {
            return false;
        }
        self.adopt_plugin_layout(&msg.layout);
        true
    }

    /// Put a result into the show, if it still belongs there. Returns whether the screen changed.
    /// Does not touch the cursor or the search: the callers that run inside a larger transition
    /// (`enter_preview`, a tab restore) are not in a state where those could be judged.
    pub(super) fn install_plugin_result(&mut self, res: PluginResult) -> bool {
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
        if let Some(layout) = &res.layout {
            show.layout = layout.clone();
        }
        let id = show.id.clone();
        let mut image_failure = None;
        match (res.view, res.failure) {
            (Some(view), _) => {
                show.view = Some(view);
                show.req = res.req;
                show.phase = PluginPhase::Ready;
            }
            (None, Some(failure)) => {
                show.view = None;
                image_failure = image_failure_of(&id, &failure);
                show.phase = PluginPhase::Failed(failure);
            }
            // A picture was drawn from the view, or an action's result: the view is what it was.
            (None, None) => {
                if !matches!(res.picture, Picture::None) {
                    show.req = res.req;
                    show.phase = PluginPhase::Ready;
                }
            }
        }
        if let Some(layout) = &res.layout {
            self.adopt_layout(layout);
        }
        // A picture plugin's failure shows its reason where the picture has one.
        if self.shows_media_plugin() {
            if let Some(why) = image_failure {
                self.apply_payload(MediaPayload::ImageFailed(why));
            }
        }
        if let Picture::Decoded(payload) = res.picture {
            self.apply_payload(*payload);
        }
        if self.plugin_view_is_text() {
            self.text_view_landed();
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

    /// Apply a plugin result that came through the worker channel (the run loop, via
    /// `apply_media`). Returns whether the screen changed.
    pub(super) fn land_plugin_result(&mut self, res: PluginResult) -> bool {
        if !self.install_plugin_result(res) {
            return false;
        }
        // Only a table has a cursor and a search of its own to settle; a text view is searched
        // by the host's window reader, which a plugin landing does not touch.
        if self.shows_code_plugin() {
            return true;
        }
        // The file landed after the transition that asked for it: the cursor the tab saved may be
        // past the end of what was read, and a search in force runs on the new cells.
        self.clamp_table_cursor();
        self.rescan_table_search();
        true
    }
}
