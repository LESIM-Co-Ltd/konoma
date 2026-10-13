//! The host side of the plugin API: opening a plugin, keeping its `Session`, and handing the
//! display type it returns to the host's own widgets. **Every plugin goes through this one road**:
//! the tables (`csv`, `archive`, `spreadsheet`), the pictures (`image`, `svg`, `pdf`, `video`) and
//! the converted documents (`document`, `presentation`) alike.
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
//! - **Work groups.** A plugin whose reads can hold gigabytes names an exclusive work group in its
//!   manifest (`Manifest::work_group`; the Office plugins share `office`). The jobs of one group run
//!   one at a time; a job asked for meanwhile waits in the group's slot, a newer one replaces it,
//!   and the running one is cancelled the usual way (the generation moved). Holding `J` over a big
//!   workbook therefore starts one read and one waiting request, whatever the key repeat does.
//! - **Documents.** The Markdown, Word and presentation plugins answer with a `Document` (Word
//!   with a `Reopen`, "open this again as Markdown": the worker opens the plugin that takes the
//!   content type on the converted text, in the same job, and keeps *its* session as the show's);
//!   the worker turns the document into the host's `LoadedDocument` (`office_doc.rs`) in the same
//!   job, so the sizes of the pictures are read off the UI thread and the view itself is not kept.
//!   Their other representation (`R`: a presentation's text, a Markdown file's or Word document's
//!   source) is asked for on the spot ([`App::plugin_view_now`]). The Markdown plugin is waited
//!   for longer on the first frame ([`MARKDOWN_FIRST_FRAME_BUDGET`]): it only reads a file.
//! - **Pages.** A table plugin with pages (a workbook's sheets) is turned with
//!   [`App::plugin_turn_page`]: the grid of the page being left is released off the UI thread and
//!   the show is "Loading..." until the page lands.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use konoma_plugin::view::{DocOptions, DocumentView, Layout, PageLayout, TableView, View, ViewReq};
use konoma_plugin::{
    t, ActCx, Cancel, Failure, Input, OpenCx, Outcome, Plugin, Session, Settings, TermCaps, ViewCx,
};

use super::media_plugin::image_failure_of;
use super::*;

/// How long the UI waits for a freshly started `open` before it draws "Loading...".
pub(crate) const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(30);

/// How long the UI waits for a freshly started `open` of the Markdown plugin. Longer than for the
/// others: what it does is read a file of at most 1 MiB, which the preview always did on the UI
/// thread, so a small file is on the first frame exactly as it always was. The wait is only a
/// ceiling: past it the screen says "Loading..." and the document lands when the worker is done.
pub(crate) const MARKDOWN_FIRST_FRAME_BUDGET: Duration = Duration::from_millis(150);

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
    /// host's (`PerTab::page_count`); the sheet names of a workbook are read from here for the
    /// "Loading..." title.
    pub layout: Layout,
    /// The latest view. It stays on screen while the same file is read again.
    pub view: Option<View>,
    /// The request the view answers.
    pub req: ViewReq,
    /// Whether each declared action works, as the session said when it last came back from a
    /// job (`Session::enabled`, asked on the worker). The key handler, the footer, the help and
    /// the `Space` menu all read this one answer, so they cannot disagree, and the UI thread
    /// never waits for a session that a job holds.
    pub enabled: std::collections::HashMap<String, bool>,
}

/// The view itself, or the failure that says the plugin returned a display type it did not declare
/// in `Manifest::shows` (so the host has no screen of that type for it): the message names the
/// plugin, the type it returned and what it declared, and is drawn like any failure. A message
/// view always fits; a plugin that is given files but declares nothing (`unscreened`) has no
/// screen for any type. The host-internal plugins and the ones that take no files (an embedded
/// formula) are not asked to declare.
fn undeclared_view(
    id: &str,
    declared: &[konoma_plugin::ViewKind],
    unscreened: bool,
    view: View,
) -> Result<View, Failure> {
    let Some(got) = view.kind() else {
        return Ok(view);
    };
    let checked = !declared.is_empty() || unscreened;
    if !checked || crate::plugins::is_internal(id) || view.fits(declared) {
        return Ok(view);
    }
    let kind = got.context();
    let tr = if declared.is_empty() {
        konoma_plugin::Tr::new(format!(
            "plugin {id} returned a {kind} view but declares no `shows`"
        ))
        .ja(format!(
            "プラグイン {id} は {kind} の表示を返しましたが、shows を宣言していません"
        ))
    } else {
        let list = declared
            .iter()
            .map(|k| k.context())
            .collect::<Vec<_>>()
            .join(", ");
        konoma_plugin::Tr::new(format!(
            "plugin {id} returned a {kind} view but declares only {list}"
        ))
        .ja(format!(
            "プラグイン {id} は {kind} の表示を返しましたが、{list} しか宣言していません"
        ))
    };
    Err(Failure::unsupported(tr))
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

/// What a worker needs to turn a plugin's document into the host's container. Given to every
/// plugin that declares it shows a document; what is done with it follows the **view** the plugin
/// returns (a `Document` or a `Reopen`), not the plugin's id.
pub(super) struct DocumentJob {
    /// The plugin that takes the content type a `Reopen` names (Word hands its converted text
    /// over to it). `None` when no enabled plugin takes `markdown`: the document cannot be shown.
    pub(super) reopen: Option<super::markdown_plugin::ContentTarget>,
    /// The options for a `Document` whose origin is `Converted` (a presentation): a converted
    /// document is drawn with the Markdown settings; only what the view says is its own (no front
    /// matter, not written back) is kept. A document read from the file as it is (a Markdown
    /// file) is drawn with the options its own view carries.
    pub(super) display: DocOptions,
}

/// What a worker calls with the layout `open` learned.
pub(super) type LayoutAnnounce = Box<dyn Fn(&Layout) + Send>;

/// The state of one exclusive work group (`Manifest::work_group`): whether one of its jobs is
/// running, and the newest job asked for since.
#[derive(Default)]
pub(crate) struct GroupSlot {
    pub(super) busy: bool,
    pub(super) queued: Option<PluginWork>,
}

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
    /// Whether the plugin declares it shows pictures: its `view` must be a picture, and the
    /// picture is decoded here, on the worker.
    pub(super) expects_picture: bool,
    /// Set when the plugin's `view` is a document (the Markdown plugin, and the Word and
    /// presentation plugins): the `Document` it returns, or the `Reopen` it hands to the plugin
    /// that takes the content type, is turned into the host's [`LoadedDocument`] here, on the
    /// worker (sizes of the pictures are read here, never on the UI thread).
    pub(super) document: Option<DocumentJob>,
    /// What stops the drawing of an untrusted SVG (the generation moved on).
    pub(super) decode_cancel: Option<crate::preview::office::Cancel>,
    /// Called with the layout as soon as `open` returns, before the first `view`.
    pub(super) on_layout: Option<LayoutAnnounce>,
    /// The plugin's exclusive work group, if it names one.
    pub(super) group: Option<String>,
    /// The ids of the actions the plugin declares: the worker asks the session which of them work
    /// now before it hands the session back.
    pub(super) act_ids: Vec<String>,
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

/// The text a plugin made, written by the worker to a private temp file the host's windowed reader
/// opens. The file is removed when this is dropped, unless the host took it for the tab (which
/// then owns it): a result that is dropped unused (a stale one, a representation the host did not
/// need) leaves nothing behind.
pub(super) struct MadeText {
    path: Option<PathBuf>,
}

impl MadeText {
    /// Write `text` to a new private temp file. Blocks: call from a worker.
    pub(super) fn write(text: &str) -> std::io::Result<MadeText> {
        let path = crate::preview::command::write_private_temp(text.as_bytes())?;
        Ok(MadeText { path: Some(path) })
    }

    /// Take the file for the tab: it is no longer removed on drop.
    pub(super) fn take(mut self) -> Option<PathBuf> {
        self.path.take()
    }

    /// Test-only: where the file is.
    #[cfg(test)]
    pub(super) fn path_for_test(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Drop for MadeText {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Write the text of a text view the host windows from a file of its own (a hex dump's lines, a
/// document's source string) on the worker, so the UI thread never writes it. A string source is
/// moved into the file (the view keeps no second copy); lines are kept for their colors.
fn write_made_text(view: &mut View, plugin: &str) -> Option<std::io::Result<MadeText>> {
    let profile = crate::plugins::profile_of(plugin);
    if !(profile.document || (profile.text && !profile.file_text)) {
        return None;
    }
    let View::Text(t) = view else {
        return None;
    };
    let text = match &mut t.source {
        konoma_plugin::view::TextSource::Lines(lines) => lines
            .iter()
            .map(|l| l.plain())
            .collect::<Vec<_>>()
            .join("\n"),
        konoma_plugin::view::TextSource::Owned(s) => std::mem::take(s),
        konoma_plugin::view::TextSource::File(_) => return None,
    };
    Some(MadeText::write(&text))
}

/// What a worker reports back. Sent through the one worker channel, inside a [`MediaResult`].
pub struct PluginResult {
    pub(super) gen: u64,
    pub(super) id: String,
    /// The file the job's show was opened for. Stamped by [`PluginWork::run`] from the job, so a
    /// late answer is only given back to the show of the same file (`reclaim_plugin_session`).
    pub(super) path: PathBuf,
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
    /// The document the view held (the Word and presentation plugins), made on the worker.
    pub(super) document: Option<Box<super::office_doc::LoadedDocument>>,
    /// The work group the job ran in: it is free again once this result has arrived.
    pub(super) group: Option<String>,
    /// For each declared action, whether the returned session says it works now. Empty when no
    /// session came back.
    pub(super) enabled: Vec<(String, bool)>,
    /// The text of a text view, written to a temp file on the worker (`write_made_text`).
    pub(super) made_text: Option<std::io::Result<MadeText>>,
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
            path: PathBuf::new(),
            session: None,
            layout: None,
            view: None,
            outcome: None,
            failure: Some(failure),
            req,
            picture: Picture::None,
            document: None,
            group: None,
            enabled: Vec::new(),
            made_text: None,
        }
    }
}

impl PluginResult {
    /// Test-only: what a worker answers with: a view, or why there is none.
    #[cfg(test)]
    pub(super) fn answer(
        gen: u64,
        id: &str,
        layout: Option<Layout>,
        view: Option<View>,
        failure: Option<Failure>,
        req: ViewReq,
    ) -> PluginResult {
        PluginResult {
            gen,
            id: id.to_string(),
            path: PathBuf::new(),
            session: None,
            layout,
            view,
            outcome: None,
            failure,
            req,
            picture: Picture::None,
            document: None,
            group: None,
            enabled: Vec::new(),
            made_text: None,
        }
    }

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

/// The host's container for a plugin's document view. `display` replaces the display options of
/// a view that did not come from the Markdown plugin, keeping the two things the view itself
/// decides: whether there is front matter to show and whether a checkbox can be written back.
fn document_of(d: DocumentView, display: Option<DocOptions>) -> super::office_doc::LoadedDocument {
    let options = match display {
        Some(display) => DocOptions {
            front_matter: d.options.front_matter,
            tasks_writable: d.options.tasks_writable,
            ..display
        },
        None => d.options,
    };
    super::office_doc::LoadedDocument::from_view(
        d.markdown,
        d.truncated,
        d.resources,
        options,
        d.origin,
    )
}

/// Open the plugin that takes the content type of `r` on its text and ask it for the document.
/// Returns its session (the plugin show keeps it: the other representation is its to give) and
/// the container. A conversion cut short stays cut short in the container.
fn reopen_document(
    target: Option<&super::markdown_plugin::ContentTarget>,
    r: konoma_plugin::view::Reopen,
    lang: &str,
    cancel: &Cancel,
    term: TermCaps,
) -> Result<(Box<dyn Session>, Box<super::office_doc::LoadedDocument>), Failure> {
    let Some(target) = target.filter(|_| r.content_type == konoma_markdown::CONTENT_TYPE) else {
        return Err(Failure::unsupported(t!(
            en: "no plugin is available to show this document",
            ja: "この文書を表示するプラグインを使えません",
        )));
    };
    let truncated = r.truncated;
    let input = Input::Content {
        content_type: r.content_type,
        text: r.text,
        resources: r.resources,
        origin: r.origin,
    };
    let open_cx = OpenCx::new(&target.settings, lang)
        .with_cancel(cancel.clone())
        .with_term(term);
    let mut session = target.plugin.open(input, &open_cx)?;
    let view_cx = ViewCx::new(&target.settings, lang)
        .with_cancel(cancel.clone())
        .with_term(term);
    let View::Document(d) = session.view(&ViewReq::default(), &view_cx)? else {
        return Err(Failure::unsupported(t!(
            en: "the plugin did not return a document",
            ja: "プラグインが文書を返しませんでした",
        )));
    };
    let mut doc = document_of(d, None);
    doc.truncated |= truncated;
    Ok((session, Box::new(doc)))
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
        let path = self.path.clone();
        let mut res = konoma_base::panic::catch_silent(move || self.run_inner(cancel))
            .unwrap_or_else(|| {
                PluginResult::failed(
                    gen,
                    id,
                    req,
                    Failure::external(t!(
                        en: "the plugin stopped unexpectedly",
                        ja: "プラグインが予期せず停止しました",
                    )),
                )
            });
        res.path = path;
        res
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
            document: document_job,
            decode_cancel,
            on_layout,
            group: _,
            act_ids,
        } = self;
        let enabled_of = |session: &dyn Session| -> Vec<(String, bool)> {
            act_ids
                .iter()
                .map(|id| (id.clone(), session.enabled(id)))
                .collect()
        };
        let term = TermCaps { pixels };
        // Whether the `trusted` claim of an SVG this plugin returns is believed: by where the
        // plugin comes from, never by what it says.
        let profile = crate::plugins::profile_of(&id);
        let trust_own_svg = profile.own_svg;
        let (declared_shows, unscreened) = (crate::plugins::shows_of(&id), profile.unscreened);
        let view_of = |mut session: Box<dyn Session>, req: ViewReq| {
            let cx = ViewCx::new(&settings, &lang)
                .with_cancel(cancel.clone())
                .with_term(term)
                .with_external(external);
            let mut picture = Picture::None;
            let mut document = None;
            let mut made_text = None;
            let viewed = session
                .view(&req, &cx)
                .and_then(|v| undeclared_view(&id, &declared_shows, unscreened, v));
            let (view, failure) = match viewed {
                Ok(View::Image(iv)) => {
                    picture = match super::media_plugin::decode_image_view(
                        iv,
                        &path,
                        default_raster_px,
                        decode_cancel.as_ref(),
                        trust_own_svg,
                    ) {
                        Some(p) => Picture::Decoded(Box::new(p)),
                        None => Picture::Failed,
                    };
                    (None, None)
                }
                // "Open this again as Markdown" (a converted Word document): the plugin that takes
                // the content type is opened on the text and answers with the document; its
                // session is the one the show keeps from here on. The pictures are sized and
                // the Markdown plugin's options kept; the view itself is not kept.
                Ok(View::Reopen(r)) if document_job.is_some() => {
                    let target = document_job.as_ref().and_then(|j| j.reopen.as_ref());
                    match reopen_document(target, r, &lang, &cancel, term) {
                        Ok((opened, doc)) => {
                            session = opened;
                            document = Some(doc);
                            (None, None)
                        }
                        Err(f) => (None, Some(f)),
                    }
                }
                Ok(View::Document(d)) if document_job.is_some() => {
                    // A converted document is drawn with the Markdown settings; one read from
                    // the file as it is, with the options its own view carries.
                    let display = matches!(d.origin, konoma_plugin::Origin::Converted { .. })
                        .then(|| document_job.as_ref().map(|j| j.display.clone()))
                        .flatten();
                    document = Some(Box::new(document_of(d, display)));
                    (None, None)
                }
                Ok(_) if expects_picture => (
                    None,
                    Some(Failure::unsupported(t!(
                        en: "the plugin did not return a picture",
                        ja: "プラグインが画像を返しませんでした",
                    ))),
                ),
                Ok(mut v) => {
                    made_text = write_made_text(&mut v, &id);
                    (Some(v), None)
                }
                Err(f) => (None, Some(f)),
            };
            // Asked **after** the view: what a read learns (a workbook's sheets) is in the layout.
            let layout = session.layout();
            let enabled = enabled_of(session.as_ref());
            PluginResult {
                gen,
                id: id.clone(),
                path: PathBuf::new(),
                session: Some(session),
                layout: Some(layout),
                view,
                outcome: None,
                failure,
                req,
                picture,
                document,
                group: None,
                enabled,
                made_text,
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
                // Asked **after** the action: it may have changed what works (`w` toggles, a
                // reload makes a key valid).
                let enabled = enabled_of(session.as_ref());
                PluginResult {
                    gen,
                    id,
                    path: PathBuf::new(),
                    layout: Some(session.layout()),
                    session: Some(session),
                    view: None,
                    outcome,
                    failure,
                    req,
                    picture: Picture::None,
                    document: None,
                    group: None,
                    enabled,
                    made_text: None,
                }
            }
        }
    }
}

/// Wrap a plugin result for the one worker channel.
fn media_result(result: PluginResult) -> MediaResult {
    MediaResult {
        gen: result.gen,
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
            && !self.shows_file_text_plugin()
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

    /// The lines of the text view a plugin answered with, joined (styles dropped). `None` unless
    /// the show holds a text view made of lines or a string: a file source is the host's windowed
    /// reader's, not this.
    pub fn plugin_text_body(&self) -> Option<String> {
        match self.plugin.as_ref()?.view.as_ref()? {
            View::Text(konoma_plugin::view::TextView {
                source: konoma_plugin::view::TextSource::Lines(lines),
                ..
            }) => Some(
                lines
                    .iter()
                    .map(|l| l.plain())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            View::Text(konoma_plugin::view::TextView {
                source: konoma_plugin::view::TextSource::Owned(s),
                ..
            }) => Some(s.clone()),
            _ => None,
        }
    }

    /// The lines of the `Message` view a plugin answered with (a notice shown in place of the
    /// content), joined. `None` unless the show holds one.
    pub fn plugin_message_body(&self) -> Option<String> {
        match self.plugin.as_ref()?.view.as_ref()? {
            View::Message(m) => Some(
                m.lines
                    .iter()
                    .map(|l| l.plain())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        }
    }

    /// The message a delegated command's plugin answered with (a detached launch: "opened
    /// externally"), if it has answered.
    pub fn command_message(&self) -> Option<String> {
        let show = self.plugin.as_ref()?;
        if show.id != crate::plugins::COMMAND_ID {
            return None;
        }
        match show.view.as_ref()? {
            View::Message(m) => Some(
                m.lines
                    .iter()
                    .map(|l| l.plain())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        }
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
        // A delegated command's show is started with the media load (`start_command_plugin`),
        // which owns it; this is not the place to drop it.
        if matches!(self.tab.preview_kind, Some(PreviewKind::Command { .. })) {
            return;
        }
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
                enabled: Default::default(),
            });
            return;
        };
        let input = Input::File { path, hint };
        self.start_plugin_open(id, plugin, input, keep_previous);
    }

    /// The request a view is asked with: the size of the preview area as far as it is known, and
    /// the page the tab is on (counted from 0; a plugin without pages ignores it).
    pub(super) fn plugin_view_req(&self) -> ViewReq {
        let rows = self.tab.preview_viewport.max(1);
        // The source view is the host's own for a standalone diagram file (no plugin round trip);
        // any other `source` is a representation the plugin declared, asked for like the rest.
        let repr = self.tab.repr.clone().filter(|r| {
            !(r == super::markdown_plugin::REPR_SOURCE
                && self.host_source_view()
                && !self.holds_document())
        });
        ViewReq {
            repr,
            page: Some(self.tab.page.max(1).saturating_sub(1) as usize),
            ..ViewReq::new(80, rows)
        }
    }

    /// What a worker needs when the plugin `id` declares it shows a document (`shows(Document)`):
    /// the plugin that takes `markdown` for a `Reopen` it may answer with, and the Markdown
    /// settings for a converted `Document` it may answer with. `None` for every other plugin.
    fn document_job(&self, id: &str) -> Option<DocumentJob> {
        crate::plugins::profile_of(id)
            .document
            .then(|| DocumentJob {
                reopen: self.content_target(konoma_markdown::CONTENT_TYPE),
                display: self.cfg_doc_options(),
            })
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
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let manifest = registry.manifest(&id);
        let group = manifest.and_then(|m| m.work_group.clone());
        let act_ids = manifest
            .map(|m| m.actions.iter().map(|a| a.id.clone()).collect())
            .unwrap_or_default();
        PluginWork {
            group,
            act_ids,
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            pixels: self.terminal_draws_pixels(),
            expects_picture: crate::preview::PreviewKind::is_media_plugin_id(&id),
            document: self.document_job(&id),
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
        let same = self.plugin.as_ref().is_some_and(|p| {
            p.id == id
                    && p.path == path
                    // A document plugin's view is the host's `document`, not `view`: it is shown
                    // once the show is `Ready`.
                    && (p.view.is_some() || matches!(p.phase, PluginPhase::Ready))
        });
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
                enabled: Default::default(),
            });
        }
        self.plugin_stamp = file_stamp(&path);
        // A Markdown or Word document is asked for in its rendered form first, whatever the tab
        // chose: the document is what the source view returns to (`land_document` asks for the
        // source once it is there).
        let mut req = self.plugin_view_req();
        if req.repr.as_deref() == Some(super::markdown_plugin::REPR_SOURCE)
            && self.host_source_view()
        {
            req.repr = None;
        }
        // A document read from the file as it is (the Markdown plugin) is waited for longer; the
        // ones that convert a file work in a group and are never waited for.
        let budget = if crate::plugins::profile_of(&id).document
            && !crate::plugins::profile_of(&id).converts
        {
            MARKDOWN_FIRST_FRAME_BUDGET
        } else {
            FIRST_FRAME_BUDGET
        };
        let work = self.plugin_work(id, path, PluginJob::Open { plugin, input }, req);
        self.dispatch_plugin_work(work, Wait::Result(budget));
    }

    /// Start a delegated command (`PreviewKind::Command`) through the built-in `command` plugin,
    /// bound to the rule's template. An image result needs a graphics backend (without one the
    /// render side falls back, as it always did); a text result needs nothing. A detached rule is
    /// waited for within the first-frame budget (it only starts a program); any other runs
    /// for as long as it takes and its result comes through the channel.
    pub(super) fn start_command_plugin(
        &mut self,
        path: &Path,
        template: &str,
        render_as: Option<&str>,
        detached: bool,
    ) {
        let as_image = render_as == Some("image");
        if !detached && as_image && self.picker.is_none() {
            return;
        }
        let plugin: Arc<dyn Plugin> = Arc::new(crate::plugins::CommandPlugin::new(
            template, render_as, detached,
        ));
        let id = crate::plugins::COMMAND_ID.to_string();
        self.drop_plugin();
        self.plugin = Some(PluginShow {
            id: id.clone(),
            path: path.to_path_buf(),
            phase: PluginPhase::Loading,
            session: None,
            layout: Layout::single(),
            view: None,
            req: ViewReq::default(),
            enabled: Default::default(),
        });
        let input = Input::File {
            path: path.to_path_buf(),
            hint: None,
        };
        let req = self.plugin_view_req();
        let work = self.plugin_work(
            id,
            path.to_path_buf(),
            PluginJob::Open { plugin, input },
            req,
        );
        let wait = if detached {
            Wait::Result(FIRST_FRAME_BUDGET)
        } else {
            Wait::Nothing
        };
        self.dispatch_plugin_work(work, wait);
    }

    /// The user's language as the plugins name it.
    pub(crate) fn plugin_lang(&self) -> &'static str {
        match self.lang {
            crate::i18n::Lang::Jp => "ja",
            crate::i18n::Lang::En => "en",
        }
    }

    /// Give `work` a new generation (everything older is stale), run it, and either apply its
    /// result on the spot (no channel; or it came within the first-frame budget of a
    /// `Wait::Result`) or leave it to the run loop. Returns whether the result was applied on the
    /// spot.
    ///
    /// A plugin in an exclusive work group (`Manifest::work_group`) waits while another job of the
    /// group runs: the job is parked in the group's slot (replacing an older parked one) and
    /// started when the running one's result arrives (`release_plugin_group`). The running job is
    /// told to stop by the generation having moved.
    pub(super) fn dispatch_plugin_work(&mut self, mut work: PluginWork, wait: Wait) -> bool {
        self.bump_media_gen();
        work.gen = self.media_gen;
        self.media_loading = true;
        if self.media_tx.is_some() {
            if let Some(group) = work.group.clone() {
                let slot = self.plugin_groups.entry(group).or_default();
                if slot.busy {
                    if let Some(older) = slot.queued.replace(work) {
                        super::table_actions::discard_in_background(older);
                    }
                    return false;
                }
                slot.busy = true;
            }
        }
        self.spawn_plugin_work(work, wait)
    }

    /// Run `work` (its group, if any, is already marked busy): on a thread of its own, or on the
    /// spot without a channel.
    fn spawn_plugin_work(&mut self, mut work: PluginWork, wait: Wait) -> bool {
        let gen = work.gen;
        let shared = self.media_gen_shared.clone();
        let cancel = {
            let shared = shared.clone();
            Cancel::from_fn(move || shared.load(std::sync::atomic::Ordering::Relaxed) != gen)
        };
        work.decode_cancel = Some(crate::preview::office::Cancel::generation(
            shared.clone(),
            gen,
        ));
        let group = work.group.clone();
        #[cfg(test)]
        if group.is_some() {
            self.plugin_group_dispatches += 1;
        }
        let Some(tx) = self.media_tx.clone() else {
            // No worker attached (tests): run here, apply now.
            let result = work.run(cancel);
            self.install_plugin_result(result);
            return true;
        };
        // A job of a work group (a workbook's sheet: seconds and gigabytes) is never waited for:
        // the screen says "Loading..." and the answer comes through the channel, as it always did.
        let wait = if group.is_some() { Wait::Nothing } else { wait };
        let handoff = matches!(wait, Wait::Result(_)).then(|| Arc::new(Handoff::new()));
        let gate = matches!(wait, Wait::Layout(_)).then(|| Arc::new(LayoutGate::new()));
        if let Some(gate) = &gate {
            let (gate, tx, id) = (Arc::clone(gate), tx.clone(), work.id.clone());
            work.on_layout = Some(Box::new(move |layout: &Layout| {
                // The UI stopped waiting: the layout goes ahead of the picture through the channel.
                if let Some(layout) = gate.publish(layout.clone()) {
                    let _ = tx.send(MediaResult {
                        gen,
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
            group.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("konoma-plugin".into())
            .spawn(move || {
                let (work, handoff, gate, tx, cancel, group) = worker;
                let taken = work.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(work) = taken {
                    let mut result =
                        crate::preview::image::with_decode_ticket(ticket, || work.run(cancel));
                    result.group = group;
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
            if let Some(group) = &group {
                self.release_plugin_group(group);
            }
            return true;
        }
        match wait {
            Wait::Nothing => false,
            Wait::Result(budget) => match handoff.and_then(|h| h.wait(budget)) {
                // A job of a work group is never waited for (`wait` is `Nothing` above), so a
                // result handed over here has no group to release.
                Some(result) => {
                    self.install_plugin_result(result);
                    true
                }
                None => false,
            },
            Wait::Layout(budget) => {
                if let Some(layout) = gate.and_then(|g| g.wait(budget)) {
                    self.adopt_plugin_layout(&layout);
                }
                false
            }
        }
    }

    /// The job that held the work group `group` has delivered its result (current or not): the
    /// group is free, and the job parked behind it starts, unless the preview moved on since it
    /// was asked for (the generation changed) and it is dropped.
    pub(super) fn release_plugin_group(&mut self, group: &str) {
        let Some(slot) = self.plugin_groups.get_mut(group) else {
            return;
        };
        slot.busy = false;
        let Some(next) = slot.queued.take() else {
            return;
        };
        if next.gen != self.media_gen {
            super::table_actions::discard_in_background(next);
            return;
        }
        if let Some(slot) = self.plugin_groups.get_mut(group) {
            slot.busy = true;
        }
        self.spawn_plugin_work(next, Wait::Nothing);
    }

    /// Forget every job parked in a work group (the preview they were asked for is gone). A job
    /// that is running keeps its group busy until its result arrives.
    pub(super) fn drop_queued_plugin_work(&mut self) {
        for slot in self.plugin_groups.values_mut() {
            if let Some(job) = slot.queued.take() {
                super::table_actions::discard_in_background(job);
            }
        }
    }

    /// Ask the open session for another `view` (a new page, a new size...). Returns false when
    /// there is no session to ask (none open, or a job still holds it).
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

    /// Ask the open session for another representation (`R`) and apply the answer **at once, on
    /// this thread**, under the generation in force (nothing else in flight is cancelled by it).
    /// For the sessions that hold what the other representation is made of (the Word and
    /// presentation plugins: it is a copy of the text and a handful of `Arc`s), so the key is
    /// answered the way it always was, without a round trip through a worker; a plugin whose other
    /// representation is a real read asks with [`App::plugin_request_view`]. Returns false when
    /// there is no session to ask (none open, or a job holds it).
    pub(super) fn plugin_view_now(&mut self, req: ViewReq) -> bool {
        let Some(show) = self.plugin.as_mut() else {
            return false;
        };
        let Some(session) = show.session.take() else {
            return false;
        };
        let (id, path) = (show.id.clone(), show.path.clone());
        let mut work = self.plugin_work(id, path, PluginJob::View { session }, req);
        work.gen = self.media_gen;
        let result = work.run(Cancel::from_fn(|| false));
        self.install_plugin_result(result);
        true
    }

    /// Show the page the tab is on (`PerTab::page`) of the table plugin that is open: a workbook's
    /// next sheet. The grid of the page being left is released off the UI thread (a big sheet is
    /// hundreds of MB, and only one sheet is held at a time) and the show reads "Loading..." until
    /// the page lands; the session is reused when a job does not hold it, else the plugin is opened
    /// again (it is cheap: a page is read by `view`). Returns whether the page landed on the spot
    /// (no worker, or an answer within no time), for the caller to settle the cursor and search.
    pub(super) fn plugin_turn_page(&mut self) -> bool {
        let Some(PreviewKind::Plugin { path, id, hint }) = self.tab.preview_kind.clone() else {
            return false;
        };
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let plugin = if registry.is_enabled(&id) {
            registry.plugin(&id)
        } else {
            None
        };
        let Some(plugin) = plugin else {
            return false;
        };
        let kept = self
            .plugin
            .as_mut()
            .filter(|p| p.id == id && p.path == path)
            .map(|show| {
                if let Some(old) = show.view.take() {
                    super::table_actions::discard_in_background(old);
                }
                show.phase = PluginPhase::Loading;
                show.session.take()
            });
        let Some(session) = kept else {
            // Nothing of this file is open (the show was dropped): open it afresh, at this page.
            self.open_plugin(false);
            return false;
        };
        let job = match session {
            Some(session) => PluginJob::View { session },
            None => PluginJob::Open {
                plugin,
                input: Input::File {
                    path: path.clone(),
                    hint,
                },
            },
        };
        let work = self.plugin_work(id, path, job, self.plugin_view_req());
        self.dispatch_plugin_work(work, Wait::Result(Duration::ZERO))
    }

    /// Run a declared action of the open session. Returns false when there is no session to ask.
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
        } else if layout.pages.is_none() {
            // The content has no pages (a workbook that failed to open has no sheets to switch
            // between): the page keys have nothing to turn.
            self.tab.page_count = None;
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
        let asked = res.req.repr.clone();
        // The temp file of the text the worker wrote: used below, or removed when this is dropped.
        let made_text = res.made_text;
        let Some(show) = self.plugin.as_mut() else {
            return false;
        };
        // The session being replaced (a read again) holds the pictures of a document: it is
        // freed off the UI thread.
        let session_back = res.session.is_some();
        if let Some(old) = std::mem::replace(&mut show.session, res.session) {
            super::table_actions::discard_in_background(old);
        }
        // What works now is what the session returned from this job said. A result without a
        // session (a failure) leaves the answer as it was.
        if session_back {
            show.enabled = res.enabled.into_iter().collect();
        }
        if let Some(layout) = &res.layout {
            show.layout = layout.clone();
        }
        let id = show.id.clone();
        // A delegated command's text result is a file the host reads in windows: it becomes the
        // tab's generated output (`tab.command_out`), which the windowed reader opens.
        let command_text = match (&res.view, id == crate::plugins::COMMAND_ID) {
            (
                Some(View::Text(konoma_plugin::view::TextView {
                    source: konoma_plugin::view::TextSource::File(p),
                    ..
                })),
                true,
            ) => Some(p.clone()),
            _ => None,
        };
        let mut image_failure = None;
        // Whether this very result carried a text view (the source of a document): a text view
        // that was already there, kept while a document landed, is not "landed" again.
        let mut text_landed = false;
        match (res.view, res.failure) {
            (Some(view), _) => {
                text_landed = matches!(view, View::Text(_));
                // The view replaced may hold a big grid: it is freed off the UI thread.
                if let Some(old) = show.view.replace(view) {
                    super::table_actions::discard_in_background(old);
                }
                show.req = res.req;
                show.phase = PluginPhase::Ready;
            }
            (None, Some(failure)) => {
                if let Some(old) = show.view.take() {
                    super::table_actions::discard_in_background(old);
                }
                image_failure = image_failure_of(&id, &failure);
                show.phase = PluginPhase::Failed(failure);
            }
            // A picture was drawn from the view, a document was made from it, or an action's
            // result: the view is what it was.
            (None, None) => {
                if !matches!(res.picture, Picture::None) || res.document.is_some() {
                    // The document replaces whatever view was kept (the source text of the other
                    // representation).
                    if res.document.is_some() {
                        if let Some(old) = show.view.take() {
                            super::table_actions::discard_in_background(old);
                        }
                    }
                    show.req = res.req.clone();
                    show.phase = PluginPhase::Ready;
                }
            }
        }
        if let Some(layout) = &res.layout {
            self.adopt_layout(layout);
        }
        // The Word and presentation plugins: the document lands in the host's container (or the
        // earlier one goes when the file could not be shown).
        if let Some(doc) = res.document {
            self.land_document(doc, asked.as_deref());
        } else if self.plugin_failure().is_some() {
            self.document_failed();
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
        // The delegated command: its text output opens the windowed reader, a failure is the reason
        // the `[can not preview]` screen gives.
        if id == crate::plugins::COMMAND_ID {
            if let Some(p) = command_text {
                self.apply_payload(MediaPayload::CommandText(p));
            } else if let Some(f) = self.plugin_failure() {
                self.command_err = Some(f.message.en().to_string());
            }
        }
        if self.plugin_view_is_text() {
            self.text_view_landed();
            // The source of a Markdown file or a Word document: open the windowed reader on it.
            if text_landed && self.holds_document() {
                self.source_view_landed(made_text);
            } else if text_landed && self.text_plugin_is_generated() {
                // A plugin that makes its text (a hex dump): the reader windows what it made.
                self.made_text_landed(made_text);
            }
        }
        match outcome {
            Some(Outcome::Flash(msg)) => {
                self.flash = Some(msg.get(self.plugin_lang()).to_string());
            }
            Some(Outcome::Reload) => self.reload_plugin(),
            // The action changed what the session shows: ask it again with the same request.
            // (The result of this job is applied and the show holds the session again.)
            Some(Outcome::Redraw) => {
                if let Some(req) = self.plugin.as_ref().map(|p| p.req.clone()) {
                    self.plugin_request_view(req);
                }
            }
            Some(Outcome::Unhandled) | None => {}
        }
        true
    }

    /// Whether the preview on screen is the file `show` was opened for.
    pub(super) fn shows_plugin_file(&self, show: &PluginShow) -> bool {
        match &self.tab.preview_kind {
            Some(PreviewKind::Plugin { id, path, .. }) => *id == show.id && *path == show.path,
            // A delegated command: the host's `command` plugin, on the file the rule matched.
            Some(PreviewKind::Command { path, .. }) => {
                show.id == crate::plugins::COMMAND_ID && *path == show.path
            }
            Some(
                PreviewKind::Text(_)
                | PreviewKind::GitDiff(_)
                | PreviewKind::MermaidFence(_)
                | PreviewKind::CanNotPreview { .. },
            )
            | None => false,
        }
    }

    /// Apply a plugin result that came through the worker channel (the run loop, via
    /// `apply_media`). Returns whether the screen changed.
    pub(super) fn land_plugin_result(&mut self, res: PluginResult) -> bool {
        if !self.install_plugin_result(res) {
            return false;
        }
        // A delegated command has no table cursor or search of its own to settle: its text is
        // searched by the host's window reader.
        if matches!(self.tab.preview_kind, Some(PreviewKind::Command { .. })) {
            return true;
        }
        // Only a table has a cursor and a search of its own to settle; a text view is searched
        // by the host's window reader, which a plugin landing does not touch, and a document by
        // the Markdown pipeline.
        if self.shows_text_plugin() || self.holds_document() {
            return true;
        }
        // A read that failed ends the wait of a search confirmed meanwhile: a later successful
        // read runs the search where it stands, as if it had never been waiting.
        if self.plugin_failure().is_some() {
            self.tab.search_pending = false;
            self.clamp_table_cursor();
            return true;
        }
        // The file landed after the transition that asked for it: the cursor the tab saved may be
        // past the end of what was read, and a search in force runs on the new cells.
        self.clamp_table_cursor();
        self.rescan_table_search();
        true
    }
}
