//! The pictures of a document (`View::Document`) in the standalone app: the host side of
//! [`crate::document_pictures`], the part konoma keeps in `src/app/md_media.rs`,
//! `src/app/md_render.rs` and `src/ui/preview.rs`.
//!
//! What a picture's room is, what a request does and what a result means is the shared code
//! ([`PictureLookup`], [`DocPictures`]); this module owns the state and wires it to the loop:
//!
//! - **The layout asks, the draw never decodes.** [`DocPics::render`] lays the page out with the
//!   sizes the cache knows (a picture not there yet is `Loading`), then asks for what is missing:
//!   a remote download, a mermaid or math render. Each runs on its own thread; a result lands in
//!   [`DocPics::poll_at`], which says when the page has to be laid out again.
//! - **A local image is decoded when it is first drawn** (as konoma does): the overlay pass asks
//!   for the decode, then for the encode of the placement; the encode worker is konoma's
//!   ([`md_encode_worker`]).
//! - **Mermaid and math** are drawn by the plugin the registry names for the kind
//!   ([`EmbedRender`]). With none, they stay text (a text diagram, the raw expression), as in
//!   the host.
//! - **The pictures a document carries** (`DocumentView::resources`: a Word document's embedded
//!   images, the `office-img://` ones) are held here and decoded from memory by the shared worker
//!   ([`DocPictures::begin_resource_decode`]), as the host does. A new document (`reset`) or a
//!   different set of resources drops the old ones.
//! - **Remote images** obey `external.remote_images`. Off, a URL is marked failed at once and
//!   nothing is fetched.
//! - **Waiting is decided by state**, not by the clock: [`DocPics::loading`] is true while a
//!   download, a decode, a render or an encode is out, and only then does the loop poll.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use konoma_host_core::lang::Lang;
use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::{
    md_encode_worker, DecodedImage, InlineImageCache, MdEncodeResult, OverlayMoved,
    MD_IMAGE_CACHE_BYTES,
};
use konoma_widgets::markdown::{is_office_image_url, is_synthetic_md_url, BlockAlign};
use konoma_widgets::picture_overlay::{
    draw_inline_pictures, InlineOverlayValues, InlinePicturePlacement, PicturePaintKind,
    PicturePaintRequest,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;

use super::document::Laid;
use crate::canvas_pictures::{embed_render_for, image_failure_message, EmbedRender, SvgDraw};
use crate::document_pictures::{
    fetch_remote_image, live_synthetic_keys, mermaid_fit_rows, mermaid_rows_cap,
    office_pictures_in_flight, resolve_md_image_path, DecodeJob, DocPicture, DocPictures, Fetcher,
    PictureLookup, RemoteFetch, Requested, ResourceJob, ResourceSlots, DEFAULT_MERMAID_ROWS,
    FENCE_MERMAID, MATH, MATH_PX, MAX_SYNTHETIC_RENDERS_IN_FLIGHT, SLIDE_CHROME_ROWS,
};
use crate::document_render::{
    render_document, PictureRequests, PictureSlots, RenderSettings, RenderedDocument,
};
use crate::host::project::media_diff::{self, MediaDiffComputed, MediaDiffOutcome};
use crate::host::project::rendered_diff::{
    render_diff_document, RenderedDiffBody, RenderedDiffInput,
};
use crate::input::{Resource, Resources};
use crate::registry::{PluginOrigin, Registry};
use crate::view::{DocOptions, DocumentView};
use crate::Settings;

#[cfg(test)]
#[path = "inline_overlay_tests.rs"]
pub(super) mod inline_overlay_tests;

/// Finds the plugin that draws an embedded kind (`None`: no enabled plugin takes it).
/// The drawer for the SVGs that are not drawn in this process is the pictures' own.
pub type EmbedSource = Box<dyn Fn(&str, &Arc<dyn SvgDraw + Send + Sync>) -> Option<EmbedRender>>;

/// What the app is told about the pictures of a document besides the terminal.
pub struct DocEnv {
    /// Who draws mermaid fences and math. Without one they are text.
    pub embed: EmbedSource,
    /// Whether `http(s)://` images are fetched at all (`external.remote_images`).
    pub remote_images: bool,
    /// The download itself (a test seam: the real one calls the network).
    pub fetcher: Fetcher,
}

impl DocEnv {
    /// The environment of a program whose plugins are in `registry`: mermaid and math are drawn
    /// by the plugins it names for their kinds, with the settings `settings_of` gives (a plugin
    /// of the program's own is trusted to draw its own SVG, as in the host).
    pub fn from_registry(
        registry: Arc<Registry>,
        lang: &'static str,
        settings_of: Box<dyn Fn(&str) -> Settings>,
        remote_images: bool,
    ) -> DocEnv {
        DocEnv {
            embed: Box::new(move |kind, svg| {
                embed_render_for(
                    &registry,
                    kind,
                    lang,
                    &*settings_of,
                    &|id| {
                        matches!(
                            registry.origin(id),
                            Some(PluginOrigin::Standard | PluginOrigin::Bundled)
                        )
                    },
                    Arc::clone(svg),
                )
            }),
            remote_images,
            fetcher: fetch_remote_image,
        }
    }
}

impl Default for DocEnv {
    fn default() -> DocEnv {
        DocEnv {
            embed: Box::new(|_, _| None),
            remote_images: true,
            fetcher: fetch_remote_image,
        }
    }
}

/// The host's optional inline fence focus; source ordinals survive loading/text fallbacks.
#[derive(Clone, Copy, Debug)]
pub struct DocPictureFocus<'a> {
    pub ordinal: usize,
    pub zoom: f64,
    pub center: (f64, f64),
    pub image_align: BlockAlign,
    /// Already translated and formatted by the host.
    pub title: &'a str,
}

/// The document's picture state.
pub struct DocPics {
    cache: InlineImageCache,
    enc_rx: Receiver<MdEncodeResult>,
    res_tx: Sender<DecodedImage>,
    res_rx: Receiver<DecodedImage>,
    fetch_tx: Sender<RemoteFetch>,
    fetch_rx: Receiver<RemoteFetch>,
    remote_inflight: HashSet<String>,
    remote_failed: HashSet<String>,
    env: DocEnv,
    font: (u16, u16),
    plugin_id: String,
    path: PathBuf,
    raster_px: u32,
    svg: Arc<dyn SvgDraw + Send + Sync>,
    /// Media files must use a host's supervised drawer, never the ordinary in-process default.
    media_svg: Option<Arc<dyn SvgDraw + Send + Sync>>,
    generation: Arc<AtomicU64>,
    /// Counts the results that change what the page reserves: a cached layout of an older epoch
    /// is built again.
    epoch: u64,
    relayout: bool,
    redraw: bool,
    overlay: OverlayMoved,
    /// Whether the last frame drawn was the document (an animation needs no wake otherwise).
    on_document: bool,
    /// The resources of the document on screen (`DocumentView::resources`) and the pictures read
    /// from them.
    resources: Resources,
    pictures: HashMap<String, DocPicture>,
    /// Set by a draw of a picture that had to leave something out (shared by all its pictures).
    render_truncated: Arc<AtomicBool>,
}

/// Whether `a` and `b` hold the very same resources (the same names over the same shared data).
fn same_resources(a: &Resources, b: &Resources) -> bool {
    a.len() == b.len()
        && a.iter().all(|(k, ra)| match (ra, b.get(k)) {
            (Resource::Bytes(x), Some(Resource::Bytes(y))) => Arc::ptr_eq(x, y),
            (Resource::Lazy(x), Some(Resource::Lazy(y))) => {
                std::ptr::addr_eq(Arc::as_ptr(x), Arc::as_ptr(y))
            }
            _ => false,
        })
}

impl DocPics {
    pub(crate) fn cell_px(&self) -> (u32, u32) {
        (self.font.0.into(), self.font.1.into())
    }

    pub(crate) fn contains_media_key(&self, key: &std::path::Path) -> bool {
        self.cache.cache.contains_key(key)
    }

    /// Paint the shared media frame's reservations in one overlay pass, without a document layout.
    pub(crate) fn draw_media(
        &mut self,
        buf: &mut Buffer,
        inner: Rect,
        paints: &[(String, PicturePaintRequest)],
    ) {
        use konoma_widgets::picture_overlay::OverlaySig;
        self.on_document = true;
        self.overlay.begin();
        self.cache.begin_pass();
        let mut sig = OverlaySig::new(inner);
        for (key, r) in paints {
            sig.add(key, r.target);
            self.ensure_image(key, *r);
            if let Some(img) = self.cache.proto(
                &PathBuf::from(key),
                r.cols,
                r.rows,
                r.row_offset,
                r.visible_rows,
            ) {
                img.render(r.target, buf);
            }
        }
        if !paints.is_empty() {
            self.overlay.note(sig.finish());
        }
        self.overlay.finish();
    }
    /// Supply the host's supervised child-process drawer for retained media SVGs.
    pub fn with_media_svg(mut self, svg: Arc<dyn SvgDraw + Send + Sync>) -> Self {
        self.media_svg = Some(svg);
        self
    }

    /// Leave the media diff, releasing only its pictures from the shared document cache.
    pub fn prune_media_diff_picture_cache(&mut self) {
        self.prune_media(None);
    }

    fn prune_media(&mut self, outcome: Option<&MediaDiffOutcome>) {
        media_diff::prune_media_diff_picture_cache(&mut self.cache.cache, outcome);
        let cache = &self.cache.cache;
        self.cache.kitty_ids.retain(|key, slots| {
            let removed = konoma_widgets::media_diff::is_media_diff_url(&key.to_string_lossy())
                && !cache.contains_key(key);
            if removed {
                self.cache
                    .kitty_deletes
                    .extend(slots.iter().flatten().copied());
            }
            !removed
        });
    }

    /// Adopt a worker's pixels into the ordinary document cache before its normal encode pass.
    pub fn materialize_media_diff(&mut self, computed: MediaDiffComputed) -> MediaDiffOutcome {
        let outcome = media_diff::materialize_media_diff(computed, |p| {
            let applied = self.dp().place_media_diff_picture(p);
            if applied.pixels {
                self.cache.evict_to(MD_IMAGE_CACHE_BYTES);
            }
        });
        self.prune_media(Some(&outcome));
        outcome
    }

    /// Retarget relative picture paths, dropping the previous file's in-flight results.
    pub fn set_path(&mut self, path: &std::path::Path) {
        if self.path != path {
            self.prune_media_diff_picture_cache();
            self.reset();
            self.path = path.to_path_buf();
        }
    }

    /// Use a supplied cache and encode receiver, for hosts that own the encoder externally.
    pub fn with_cache(mut self, cache: InlineImageCache, enc_rx: Receiver<MdEncodeResult>) -> Self {
        self.cache = cache;
        self.enc_rx = enc_rx;
        self
    }

    pub fn new(
        picker: &Picker,
        plugin_id: &str,
        path: &std::path::Path,
        raster_px: u32,
        svg: Arc<dyn SvgDraw + Send + Sync>,
    ) -> DocPics {
        let (enc_tx, req_rx) = channel();
        let (enc_res_tx, enc_rx) = channel();
        let mut cache = InlineImageCache::default();
        cache.attach_picker(picker);
        cache.enc_tx = Some(enc_tx);
        let p = picker.clone();
        std::thread::spawn(move || md_encode_worker(p, req_rx, enc_res_tx));
        let (res_tx, res_rx) = channel();
        let (fetch_tx, fetch_rx) = channel();
        DocPics {
            cache,
            enc_rx,
            res_tx,
            res_rx,
            fetch_tx,
            fetch_rx,
            remote_inflight: HashSet::new(),
            remote_failed: HashSet::new(),
            env: DocEnv::default(),
            font: {
                let f = picker.font_size();
                (f.width, f.height)
            },
            plugin_id: plugin_id.to_string(),
            path: path.to_path_buf(),
            raster_px,
            svg,
            media_svg: None,
            generation: Arc::new(AtomicU64::new(0)),
            epoch: 0,
            relayout: false,
            redraw: false,
            overlay: OverlayMoved::new(),
            on_document: false,
            resources: Resources::new(),
            pictures: HashMap::new(),
            render_truncated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Hold the pictures `resources` name (the document on screen's). The very same resources as
    /// before change nothing; others mean another document (or the same one read again), so
    /// the old document is reset (decodes cancelled, kitty images deleted, cached document
    /// pictures dropped). Worker-adopted media diff keys belong to their own lifecycle.
    fn sync_resources(&mut self, resources: &Resources) {
        if same_resources(&self.resources, resources) {
            return;
        }
        self.reset();
        self.render_truncated = Arc::new(AtomicBool::new(false));
        self.pictures = DocPicture::from_resources(resources, &self.render_truncated);
        self.resources = resources.clone();
    }

    /// Seed ready protocols without scheduling worker jobs in cross-host adapter tests.
    #[cfg(feature = "test-hooks")]
    pub fn cache_for_test(&mut self) -> &mut InlineImageCache {
        &mut self.cache
    }

    /// Adopt one decode at a deterministic result-channel fence in adapter tests.
    #[cfg(feature = "test-hooks")]
    pub fn await_decode_for_test(&mut self) {
        let res = self
            .res_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("decode result");
        self.apply_decoded(res);
    }

    /// Inspect the signature recorded by the real draw entry point in adapter tests.
    #[cfg(feature = "test-hooks")]
    pub fn overlay_for_test(&self) -> &OverlayMoved {
        &self.overlay
    }

    pub fn set_env(&mut self, env: DocEnv) {
        self.env = env;
    }

    /// Whether drawing a picture had to leave something out.
    pub fn render_truncated(&self) -> bool {
        self.render_truncated
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The count a cached layout is compared with.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether the page must be laid out again since this was last asked (resets the latch).
    pub fn take_relayout(&mut self) -> bool {
        std::mem::take(&mut self.relayout)
    }

    /// Pixels changed without changing the document's reserved layout or asking its session again.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    /// The most rows a mermaid fence may take for a viewport of `rows` rows (the host's
    /// `mermaid_fit_rows`).
    pub fn fence_rows(&self, rows: u16) -> u16 {
        let raw = (self.env.embed)(FENCE_MERMAID, &self.svg)
            .map_or(i64::from(DEFAULT_MERMAID_ROWS), |e| {
                e.settings().int("rows", i64::from(DEFAULT_MERMAID_ROWS))
            });
        mermaid_fit_rows(mermaid_rows_cap(raw), rows)
    }

    /// Lay the document out at `width` cells (`rows` is the viewport's height): the shared
    /// renderer with the sizes the cache holds, then the requests for what is missing. A request
    /// that was answered at once (remote images off, no results channel) lays the page out once
    /// more, so the page never shows a picture waiting for something that will not come.
    pub fn render(
        &mut self,
        dv: &DocumentView,
        markdown: &str,
        width: u16,
        rows: u16,
        lang: Lang,
        settings: &RenderSettings<'_>,
    ) -> RenderedDocument {
        self.sync_resources(&dv.resources);
        self.render_with(
            width,
            rows,
            lang,
            |slots| render_document(markdown, &dv.options, settings, slots),
            RenderedDocument::requests,
        )
    }

    /// Lay out a worker-prepared diff through the ordinary document's slot and request loop.
    /// Both versions' requests participate in the same sweep, resync and background workers.
    pub fn render_diff(
        &mut self,
        input: &RenderedDiffInput<'_>,
        options: &DocOptions,
        settings: &RenderSettings<'_>,
        rows: u16,
        lang: Lang,
    ) -> RenderedDiffBody {
        self.render_with(
            settings.width,
            rows,
            lang,
            |slots| render_diff_document(input, options, settings, slots),
            RenderedDiffBody::requests,
        )
    }

    fn render_with<T>(
        &mut self,
        width: u16,
        rows: u16,
        lang: Lang,
        render: impl Fn(&PictureSlots<'_>) -> T,
        requests: for<'a> fn(&'a T) -> PictureRequests<'a>,
    ) -> T {
        let fence = (self.env.embed)(FENCE_MERMAID, &self.svg);
        let math = (self.env.embed)(MATH, &self.svg);
        let image_mode = |e: &Option<EmbedRender>| {
            e.as_ref()
                .is_some_and(|e| e.settings().str("mode", "image") != "text")
        };
        let (mermaid_on, math_on) = (image_mode(&fence), image_mode(&math));
        let fence_rows = self.fence_rows(rows);
        let base_dir = self.path.parent().map(|p| p.to_path_buf());
        let mut pass = 0;
        loop {
            let rendered = {
                let refusals = RefCell::new(Vec::new());
                let message = |f: ImageFailure, pid: &str| image_failure_message(f, lang, pid);
                let own_message = |f: ImageFailure| message(f, &self.plugin_id);
                let font = self.font;
                let resource_slots = ResourceSlots {
                    pictures: &self.pictures,
                    cache: &self.cache,
                    font,
                    avail: width.saturating_sub(2),
                    slide_rows: rows.saturating_sub(SLIDE_CHROME_ROWS).max(1),
                    message: &own_message,
                };
                let lookup = PictureLookup {
                    cache: &self.cache,
                    remote_failed: &self.remote_failed,
                    font: Some(self.font),
                    avail: width.saturating_sub(2),
                    base_dir: base_dir.clone(),
                    fence_target_rows: fence_rows,
                    mermaid_on,
                    math_on,
                    plugin_id: &self.plugin_id,
                    refused: &|_, _| None,
                    refusals: &refusals,
                    message: &message,
                    other: &|url, max_cols| resource_slots.slot(url, max_cols),
                };
                lookup.with_slots(true, &render)
            };
            pass += 1;
            let resync = self.request_all(requests(&rendered), fence.clone(), math.clone());
            if !resync || pass >= 2 {
                return rendered;
            }
        }
    }

    /// The requests a laid-out page makes. True when one was answered at once.
    fn request_all(
        &mut self,
        r: PictureRequests<'_>,
        fence: Option<EmbedRender>,
        math: Option<EmbedRender>,
    ) -> bool {
        let mut resync = false;
        let (raster_px, results, fetches) =
            (self.raster_px, self.res_tx.clone(), self.fetch_tx.clone());
        let (remote_images, fetcher) = (self.env.remote_images, self.env.fetcher);
        let mut dp = DocPictures {
            cache: &mut self.cache,
            remote_inflight: &mut self.remote_inflight,
            remote_failed: &mut self.remote_failed,
            results: Some(results),
            fetches: Some(fetches),
            remote_images,
            fetcher,
        };
        for url in r.remote_urls {
            resync |= dp.request_remote(url);
        }
        for code in r.mermaid_fences {
            if code.trim().is_empty() {
                continue;
            }
            let key = PathBuf::from(konoma_widgets::markdown::mermaid_fence_url(code));
            if dp.is_cached(&key) {
                continue;
            }
            resync |= dp.request_fence(code.clone(), fence.clone(), FENCE_MERMAID, raster_px)
                == Requested::Done;
        }
        for (latex, display) in r.math_exprs {
            if latex.trim().is_empty() {
                continue;
            }
            let key = PathBuf::from(konoma_widgets::markdown::math_url(latex, *display));
            if dp.is_cached(&key) {
                continue;
            }
            resync |= dp.request_math(latex.clone(), *display, math.clone(), MATH, MATH_PX)
                == Requested::Done;
        }
        // A fence or an expression that left the document is dropped (its key is a content hash).
        dp.sweep(&live_synthetic_keys(r.mermaid_fences, r.math_exprs));
        resync
    }

    /// Apply what the workers finished. A result that changes what the page reserves asks for the
    /// layout again ([`Self::take_relayout`]).
    pub fn poll_at(&mut self, now: std::time::Instant) {
        while let Ok(res) = self.enc_rx.try_recv() {
            self.cache.apply_encode(res);
            self.redraw = true;
        }
        while let Ok(res) = self.fetch_rx.try_recv() {
            self.dp().apply_fetch(res);
            self.bump();
        }
        while let Ok(res) = self.res_rx.try_recv() {
            self.apply_decoded(res);
        }
        self.redraw |= self.cache.advance_gifs_at(now);
    }

    /// The document is another one now: its own pictures go, while media-diff pictures stay
    /// under their separate owner. The document decodes in flight are
    /// cancelled (their generation moves on), the channels are new ones (a result they were to
    /// send lands on nothing), the cached pictures are dropped with the kitty images they pinned,
    /// and the page is laid out again.
    pub fn reset(&mut self) {
        self.reset_document();
    }

    fn apply_decoded(&mut self, res: DecodedImage) {
        let applied = self.dp().apply_result(res);
        self.redraw |= applied.redraw;
        if applied.relayout {
            self.bump();
        }
        if applied.pixels {
            self.cache.evict_to(MD_IMAGE_CACHE_BYTES);
        }
    }

    fn reset_document(&mut self) {
        let old = std::mem::replace(&mut self.generation, Arc::new(AtomicU64::new(0)));
        old.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (self.res_tx, self.res_rx) = channel();
        (self.fetch_tx, self.fetch_rx) = channel();
        self.remote_inflight.clear();
        self.remote_failed.clear();
        self.resources = Resources::new();
        self.pictures.clear();
        let retained =
            |key: &PathBuf| konoma_widgets::media_diff::is_media_diff_url(&key.to_string_lossy());
        let pinned: HashSet<u32> = self
            .cache
            .kitty_ids
            .iter()
            .filter(|(key, _)| !retained(key))
            .flat_map(|(_, slots)| slots.iter().flatten().copied())
            .collect();
        self.cache.kitty_deletes.extend(pinned);
        self.cache.kitty_ids.retain(|key, _| retained(key));
        self.cache.cache.retain(|key, entry| {
            if !retained(key) {
                return false;
            }
            // The old result channel was dropped; an evicted SVG can request its child again.
            entry.rebuilding = false;
            entry.reraster_inflight = false;
            true
        });
        self.bump();
    }

    fn bump(&mut self) {
        self.epoch += 1;
        self.relayout = true;
    }

    fn dp(&mut self) -> DocPictures<'_> {
        DocPictures {
            cache: &mut self.cache,
            remote_inflight: &mut self.remote_inflight,
            remote_failed: &mut self.remote_failed,
            results: Some(self.res_tx.clone()),
            fetches: Some(self.fetch_tx.clone()),
            remote_images: self.env.remote_images,
            fetcher: self.env.fetcher,
        }
    }

    /// Whether pixel work is owed to the UI thread: a download, a decode or render without its
    /// result, or an encode in flight.
    pub fn loading(&self) -> bool {
        !self.remote_inflight.is_empty()
            || self.cache.encodes_in_flight()
            || self
                .cache
                .cache
                .values()
                .any(|e| e.rebuilding || (e.decoded.is_none() && !e.failed && !e.evicted))
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn settle_for_test(&mut self) {
        while self.loading() {
            if !self.remote_inflight.is_empty() {
                let result = self
                    .fetch_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("project idle fence: document picture fetch");
                self.dp().apply_fetch(result);
                self.bump();
            } else if self.cache.encodes_in_flight() {
                let result = self
                    .enc_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("project idle fence: document picture encode");
                self.cache.apply_encode(result);
                self.redraw = true;
            } else {
                let result = self
                    .res_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("project idle fence: document picture decode");
                self.apply_decoded(result);
            }
        }
    }

    /// How long the loop may wait: a short poll while something is out, the next GIF frame while
    /// the document is on screen.
    pub fn wake_in(&self, busy_poll: Duration) -> Option<Duration> {
        self.wake_in_at(busy_poll, std::time::Instant::now())
    }

    pub fn wake_in_at(&self, busy_poll: Duration, now: std::time::Instant) -> Option<Duration> {
        let mut wake = self.loading().then_some(busy_poll);
        if self.on_document {
            if let Some(g) = self.cache.gif_poll_timeout_at(now) {
                wake = Some(wake.map_or(g, |w| w.min(g)));
            }
        }
        wake
    }

    /// Whether the pictures moved or went since this was last asked.
    pub fn take_moved(&mut self) -> bool {
        self.overlay.take_moved()
    }

    /// The deletes of the kitty images that went.
    pub fn take_kitty_deletes(&mut self) -> Vec<String> {
        self.cache.take_kitty_deletes()
    }

    /// Draw the pictures of the laid-out page over the cells already painted in `inner` (the
    /// inside of the frame's border). `doc` is the page and its scroll in display rows; `None`
    /// is a frame that is not the document, which reports that the pictures left.
    pub fn draw(&mut self, buf: &mut Buffer, inner: Rect, doc: Option<(&Laid, usize)>) {
        self.draw_with(buf, inner, doc, 0, None);
    }

    /// Draw with the host's gutter and optional source-fence focus. File apps use `draw`'s defaults.
    pub fn draw_with(
        &mut self,
        buf: &mut Buffer,
        inner: Rect,
        doc: Option<(&Laid, usize)>,
        gutter: u16,
        focus: Option<DocPictureFocus<'_>>,
    ) {
        self.on_document = doc.is_some();
        self.overlay.begin();
        if let Some((laid, scroll)) = doc {
            if inner.width > 0 && inner.height > 0 && !laid.images.is_empty() {
                if let Some(sig) = self.draw_images(buf, inner, laid, scroll, gutter, focus) {
                    self.overlay.note(sig);
                }
            }
        }
        self.overlay.finish();
    }

    fn draw_images(
        &mut self,
        buf: &mut Buffer,
        inner: Rect,
        laid: &Laid,
        scroll: usize,
        gutter: u16,
        focus: Option<DocPictureFocus<'_>>,
    ) -> Option<u64> {
        self.cache.begin_pass();
        let placements = laid.images.iter().map(|p| InlinePicturePlacement {
            url: &p.url,
            visual_row: laid.first_row_of(p.line),
            col: p.col,
            cols: p.cols,
            rows: p.rows,
            fence_ord: p.fence_ord,
        });
        let values = InlineOverlayValues {
            inner,
            scroll,
            gutter,
            focused_ordinal: focus.map(|f| f.ordinal),
            zoom: focus.map_or(1.0, |f| f.zoom),
            image_align: focus.map_or(BlockAlign::Center, |f| f.image_align),
            focus_title: focus.map_or("", |f| f.title),
        };
        draw_inline_pictures(buf, placements, values, |url, r, buf| match r.kind {
            PicturePaintKind::Density | PicturePaintKind::Zoom => false,
            PicturePaintKind::UnzoomedFallback => {
                if let Some(img) = self
                    .cache
                    .cache
                    .get(&PathBuf::from(url))
                    .and_then(|e| e.newest_full())
                {
                    img.render(r.target, buf);
                    true
                } else {
                    false
                }
            }
            PicturePaintKind::Normal => {
                self.ensure_image(url, r);
                let Some(path) = self.path_of(url) else {
                    return false;
                };
                if let Some(img) =
                    self.cache
                        .proto(&path, r.cols, r.rows, r.row_offset, r.visible_rows)
                {
                    img.render(r.target, buf);
                    true
                } else {
                    false
                }
            }
        })
    }

    /// The cache key of a placement's picture: a fence or an expression is filed under its
    /// synthetic URL, a file under its resolved path.
    fn path_of(&self, url: &str) -> Option<PathBuf> {
        if is_synthetic_md_url(url) {
            Some(PathBuf::from(url))
        } else {
            resolve_md_image_path(url, self.path.parent())
        }
    }

    /// Starts the first decode of the document's own picture `url` (filed under `key`). At most
    /// [`MAX_SYNTHETIC_RENDERS_IN_FLIGHT`] at once, like the host: skipping leaves no entry, so
    /// the next draw (a landed decode always causes one) asks again. A picture the document does
    /// not hold is left uncached (nothing to draw).
    fn begin_office_decode(&mut self, url: &str, key: PathBuf) {
        let Some(source) = self.pictures.get(url).map(|p| p.source.clone()) else {
            return;
        };
        if office_pictures_in_flight(&self.cache) >= MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
            return;
        }
        let job = ResourceJob {
            source,
            svg_max_px: self.raster_px,
            retry_floor: self.raster_px,
            reraster: false,
            svg: Arc::clone(&self.svg),
            generation: (0, Arc::clone(&self.generation)),
        };
        self.dp().begin_resource_decode(key, job);
    }

    /// Make the picture ready to be drawn: start its decode the first time it is drawn,
    /// stamp it as wanted by this pass, and ask for the encode of this placement.
    fn ensure_image(&mut self, url: &str, r: PicturePaintRequest) {
        let Some(path) = self.path_of(url) else {
            return;
        };
        if !self.cache.cache.contains_key(&path) {
            // A picture the document carries: decoded from the bytes it holds, never from a file.
            if is_office_image_url(url) {
                self.begin_office_decode(url, path);
                return;
            }
            // A synthetic picture is filed by its layout request: never decode `math://` as a file.
            if is_synthetic_md_url(url) {
                return;
            }
            let job = DecodeJob {
                svg_max_px: self.raster_px,
                roots: None,
                read_path: Some(path.clone()),
                svg: Arc::clone(&self.svg),
                generation: (0, Arc::clone(&self.generation)),
            };
            self.dp().begin_decode(path, job);
            return;
        }
        self.cache.stamp_drawn(&path);
        if konoma_widgets::media_diff::is_media_diff_url(url) {
            self.rebuild_media_svg(&path);
        }
        self.cache
            .ensure_encode(&path, r.cols, r.rows, r.row_offset, r.visible_rows, None);
    }

    fn rebuild_media_svg(&mut self, path: &std::path::Path) {
        let Some(drawer) = self.media_svg.clone() else {
            return;
        };
        let generation = self.generation.clone();
        let max_px = self.raster_px;
        self.dp()
            .rebuild_media_svg(path, max_px, drawer, (0, generation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Failure, Manifest, Session, Value};

    #[test]
    fn preview_and_media_share_cache_without_cross_pruning() {
        use konoma_widgets::inline_images::{EncodeApplied, MdEncodeKey, MdImgEntry};
        let mut d = pics(None);
        d.cache.use_kitty = true;
        let media = PathBuf::from("media-diff://same-path/old/1");
        let entry = MdImgEntry::default();
        let ticket = entry.wish.ticket();
        d.cache.cache.insert(media.clone(), entry);
        d.cache.next_request(&media);
        let born = d.cache.cache[&media].born;
        d.cache
            .kitty_ids
            .insert(media.clone(), [vec![31], vec![], vec![]]);
        let docs = ["/x/p.png", "math://expression", "mermaid-fence://diagram"];
        for (i, key) in docs.iter().enumerate() {
            d.cache
                .cache
                .insert(PathBuf::from(key), MdImgEntry::default());
            d.cache
                .kitty_ids
                .insert(PathBuf::from(key), [vec![40 + i as u32], vec![], vec![]]);
        }
        // An ordinary Preview reset owns only document pictures, even on the same path.
        d.reset();
        assert!(
            d.cache.cache.contains_key(&media),
            "ordinary reset retains media"
        );
        assert!(!ticket.is_stale());
        assert_eq!(
            d.cache.kitty_ids[&media][0],
            vec![31],
            "image numbering survives Preview"
        );
        assert_eq!(
            d.take_kitty_deletes().len(),
            3,
            "each document image deleted once"
        );
        assert!(d.take_kitty_deletes().is_empty());
        d.sync_resources(&resources_of(&stripes()));
        assert!(
            d.cache.cache.contains_key(&media),
            "document sync retains media"
        );
        for key in docs {
            d.cache.cache.insert(key.into(), MdImgEntry::default());
        }
        d.prune_media_diff_picture_cache();
        assert!(ticket.is_stale(), "departure cancels the media decode");
        assert!(
            docs.iter()
                .all(|k| d.cache.cache.contains_key(&PathBuf::from(k))),
            "media prune retains document, math and Mermaid"
        );
        assert_eq!(
            d.take_kitty_deletes().len(),
            1,
            "media owner deletes its kitty image once"
        );
        let late = || MdEncodeResult {
            path: media.clone(),
            key: MdEncodeKey::Full { cols: 4, rows: 2 },
            image: None,
            born,
        };
        assert_eq!(
            d.cache.apply_encode(late()),
            EncodeApplied::Dropped,
            "late encode after q is ignored"
        );
        let (late_tx, late_rx) = channel();
        d.enc_rx = late_rx;
        late_tx.send(late()).unwrap();
        d.poll_at(std::time::Instant::now());
        assert!(
            !d.cache.cache.contains_key(&media),
            "late encode after q is not adopted by the owner"
        );
        d.prune_media_diff_picture_cache();
        assert!(
            d.take_kitty_deletes().is_empty(),
            "a repeated departure never deletes twice"
        );
        // Returning to the same URL cannot adopt an encode of its previous incarnation.
        d.cache.cache.insert(media.clone(), MdImgEntry::default());
        d.cache.next_request(&media);
        assert_eq!(d.cache.apply_encode(late()), EncodeApplied::Dropped);
    }

    struct Nothing;

    impl crate::Plugin for Nothing {
        fn manifest(&self) -> Manifest {
            Manifest::new("nothing", "0.1.0")
        }
        fn open(&self, _: crate::Input, _: &crate::OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::corrupt(crate::Tr::new("no")))
        }
    }

    fn pics(rows: Option<i64>) -> DocPics {
        pics_with(rows.map(|r| Settings::new().with("rows", Value::Int(r))))
    }

    /// A mermaid plugin is there, with `settings` (`None`: no plugin).
    fn pics_with(settings: Option<Settings>) -> DocPics {
        let mut d = DocPics::new(
            &Picker::halfblocks(),
            "t",
            std::path::Path::new("/x/d.md"),
            800,
            Arc::new(super::super::pictures::InProcessSvg),
        );
        d.set_env(DocEnv {
            embed: Box::new(move |kind, svg| {
                (kind == FENCE_MERMAID)
                    .then_some(settings.clone())
                    .flatten()
                    .map(|settings| {
                        EmbedRender::new(Arc::new(Nothing), settings, "en", false, Arc::clone(svg))
                    })
            }),
            ..DocEnv::default()
        });
        d
    }

    #[test]
    fn a_fences_rows_come_from_the_mermaid_plugins_rows_setting() {
        assert_eq!(pics(Some(9)).fence_rows(40), 9);
        assert_eq!(pics(Some(9)).fence_rows(0), 9);
        assert_eq!(pics(Some(100)).fence_rows(20), 18);
        assert_eq!(pics(Some(0)).fence_rows(40), 24, "0 is the default");
        assert_eq!(pics(Some(-3)).fence_rows(40), 24);
        assert_eq!(pics(None).fence_rows(40), 24, "no plugin: the default");
        assert_eq!(pics(Some(9)).fence_rows(5), 4, "never under four rows");
        // A plugin that sets no `rows` at all is the default too (not a cap of 0 or a guess).
        assert_eq!(pics_with(Some(Settings::new())).fence_rows(40), 24);
        assert_eq!(pics_with(Some(Settings::new())).fence_rows(10), 8);
    }

    #[test]
    fn reset_cancels_the_old_jobs_and_drops_what_the_old_document_asked_for() {
        let mut d = pics(None);
        let old_gen = Arc::clone(&d.generation);
        let stale_tx = d.res_tx.clone();
        let stale_fetch = d.fetch_tx.clone();
        d.remote_inflight.insert("http://x/a.png".into());
        d.remote_failed.insert("http://x/b.png".into());
        d.cache
            .cache
            .insert(PathBuf::from("/x/p.png"), Default::default());
        d.cache.kitty_ids.insert(
            PathBuf::from("/x/p.png"),
            [vec![11, 12], Vec::new(), vec![13]],
        );
        d.cache.kitty_deletes = vec![7];
        let (epoch, _) = (d.epoch, d.relayout = false);

        d.reset();

        // The jobs of the old document see their generation move on; the new one starts at 0.
        assert_eq!(old_gen.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(d.generation.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!Arc::ptr_eq(&old_gen, &d.generation));
        // A result the old document was to receive lands on nothing.
        let _ = stale_tx.send(DecodedImage {
            path: PathBuf::from("/x/p.png"),
            image: Err("late".into()),
            svg: None,
            reraster: false,
            frames: None,
            request: 0,
        });
        let _ = stale_fetch.send(RemoteFetch {
            url: "http://x/a.png".into(),
            ok: true,
        });
        assert!(d.res_rx.try_recv().is_err());
        assert!(d.fetch_rx.try_recv().is_err());
        assert!(d.remote_inflight.is_empty());
        assert!(d.remote_failed.is_empty());
        assert!(d.cache.cache.is_empty());
        // The kitty images the old pictures pinned are deleted from the terminal (the ones
        // already due stay due) and their ids are not kept for the new document.
        let mut due = d.cache.kitty_deletes.clone();
        due.sort_unstable();
        assert_eq!(due, vec![7, 11, 12, 13]);
        assert!(d.cache.kitty_ids.is_empty());
        assert!(d.relayout, "the page is laid out again");
        assert_eq!(d.epoch, epoch + 1, "a cached layout is built again");
    }

    fn stripes() -> Arc<[u8]> {
        let mut out = Vec::new();
        image::RgbaImage::new(8, 8)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        Arc::from(out)
    }

    fn resources_of(bytes: &Arc<[u8]>) -> Resources {
        let mut r = Resources::new();
        r.insert("office-img://h/a.png", Resource::Bytes(bytes.clone()));
        r
    }

    #[test]
    fn the_resources_of_the_document_are_held_and_a_new_document_drops_them() {
        let mut d = pics(None);
        let bytes = stripes();
        let r = resources_of(&bytes);
        d.sync_resources(&r);
        assert_eq!(d.pictures.len(), 1);
        assert!(d.pictures["office-img://h/a.png"].dims.is_some());
        // The very same resources again: the pictures are not made again.
        let held = d.render_truncated.clone();
        d.sync_resources(&r.clone());
        assert!(Arc::ptr_eq(&held, &d.render_truncated));
        // The page was laid out again (a decode landed): the entry stays.
        d.cache
            .cache
            .insert(PathBuf::from("office-img://h/a.png"), Default::default());
        d.sync_resources(&r);
        assert_eq!(d.cache.cache.len(), 1);
        // Other resources: the whole old document goes through `reset` (every cache entry, the
        // kitty images it pinned, the decodes in flight).
        d.cache
            .cache
            .insert(PathBuf::from("/x/other.png"), Default::default());
        d.cache.kitty_ids.insert(
            PathBuf::from("office-img://h/a.png"),
            [vec![21, 22], Vec::new(), Vec::new()],
        );
        let old_gen = Arc::clone(&d.generation);
        let other = stripes();
        d.sync_resources(&resources_of(&other));
        assert!(!Arc::ptr_eq(&held, &d.render_truncated));
        assert!(!d
            .cache
            .cache
            .contains_key(&PathBuf::from("office-img://h/a.png")));
        assert!(!d.cache.cache.contains_key(&PathBuf::from("/x/other.png")));
        assert_eq!(old_gen.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!Arc::ptr_eq(&old_gen, &d.generation));
        assert!(d.cache.kitty_ids.is_empty());
        let mut due = d.cache.kitty_deletes.clone();
        due.sort_unstable();
        assert_eq!(due, vec![21, 22]);
        assert_eq!(d.pictures.len(), 1, "the new resources are held");
        // A reopened document: nothing of the old one is held.
        d.reset();
        assert!(d.pictures.is_empty());
        assert!(d.resources.is_empty());
    }

    #[test]
    fn a_picture_the_document_does_not_hold_starts_no_decode() {
        let mut d = pics(None);
        d.begin_office_decode(
            "office-img://h/none.png",
            PathBuf::from("office-img://h/none.png"),
        );
        assert!(d.cache.cache.is_empty());
        let bytes = stripes();
        d.sync_resources(&resources_of(&bytes));
        d.begin_office_decode(
            "office-img://h/a.png",
            PathBuf::from("office-img://h/a.png"),
        );
        assert_eq!(d.cache.cache.len(), 1);
        assert!(d.loading(), "a decode is out");
    }

    #[test]
    fn rendered_diff_uses_the_document_request_loop_for_both_versions_and_resyncs() {
        use crate::document_pictures::{
            clear_test_cache_root, get_test_cache_root, set_test_cache_root,
        };
        use konoma_widgets::markdown::diff_align;
        struct RestoreCache(Option<PathBuf>);
        impl Drop for RestoreCache {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(path) => set_test_cache_root(path),
                    None => clear_test_cache_root(),
                }
            }
        }
        let dir = konoma_test_support::unique_tmp("rendered_diff_picture_requests");
        let _cache = RestoreCache(get_test_cache_root());
        set_test_cache_root(dir.to_path_buf());
        let old = "![old](https://invalid.test/old.png)\n";
        let new = "![new](https://invalid.test/new.png)\n";
        let (old_doc, new_doc, ops) = diff_align(old, new);
        let input = RenderedDiffInput {
            old_pre: old,
            new_pre: new,
            old: old_doc,
            new: new_doc,
            ops: &ops,
        };
        let mut d = pics(None);
        d.set_env(DocEnv {
            remote_images: false,
            fetcher: |_, _| panic!("disabled images must never reach the network"),
            ..DocEnv::default()
        });
        let details = HashMap::new();
        let settings = RenderSettings {
            width: 32,
            code: Default::default(),
            code_theme: "base16-ocean.dark",
            icons: false,
            mermaid_caption: "diagram",
            details_open: &details,
        };
        let body = d.render_diff(&input, &DocOptions::default(), &settings, 12, Lang::En);
        assert_eq!(
            body.remote_urls,
            [
                "https://invalid.test/old.png",
                "https://invalid.test/new.png"
            ]
        );
        assert_eq!(
            d.remote_failed.len(),
            2,
            "both requests reached the ordinary loop"
        );
        assert!(body
            .remote_urls
            .iter()
            .all(|url| d.remote_failed.contains(url)));
        assert!(d.remote_inflight.is_empty());
        assert!(!d.loading());
        assert!(
            !body
                .lines
                .iter()
                .any(|line| line.to_string().contains("loading")),
            "synchronous refusal resyncs the body"
        );
        // A later identical body uses the same failures without issuing another request.
        let next = d.render_diff(&input, &DocOptions::default(), &settings, 12, Lang::En);
        assert_eq!(body.lines, next.lines);
    }
}
