//! The document container and the Office documents: a Markdown file (the `markdown` plugin), a
//! Word document and a presentation (the `document` and `presentation` plugins in
//! `crates/konoma-office`, which convert the file to Markdown with its pictures) are all drawn by
//! the host with the ordinary Markdown pipeline, from the document the plugin returned. Nothing is
//! written back to a converted file; a Markdown file's checkboxes are (`markdown_plugin.rs` holds
//! the options and the representations).
//!
//! The job runs on the plugin road (`plugin_host.rs`; the Office plugins in the Office work group:
//! one read at a time across the spreadsheet, Word and slide plugins, the newest request waiting).
//! On the worker the plugin's view (a `Document` for a Markdown file and for slides; for Word a
//! `Reopen` that the plugin taking `markdown` answers with a `Document`) is turned into a
//! [`LoadedDocument`], the host's container for the document on screen; this module holds it, the
//! App-level accessors the renderer and key handlers share, and the **anchored pages** of a
//! presentation (`Layout.pages = Anchored`: one long document whose pages start at the headings of
//! a level; `J`/`K`, the chip, the scroll limit) with the **representations** the plugin offers
//! (`Layout.reprs`: slide pictures / text; `R`).

use super::*;
use konoma_plugin::input::{Origin, Resource, Resources};
use konoma_plugin::view::{DocOptions, Layout, PageLayout};
use konoma_plugin::LazyResource;
use std::collections::HashMap;

/// Where the pixels of a picture of the open document come from.
#[derive(Clone)]
pub(super) enum PictureSource {
    /// A file inside the package (png, jpeg, gif, bmp, webp, tiff or svg): its bytes, shared with
    /// decode threads (and with the plugin's session).
    Bytes(Arc<[u8]>),
    /// A picture the plugin makes when asked (a slide of a presentation): no bytes exist yet. The
    /// decode thread asks it for an SVG and hands that to the untrusted-SVG drawing path.
    Lazy {
        lazy: Arc<dyn LazyResource>,
        /// Set by the decode thread when drawing the picture had to leave something out (a size
        /// budget of the writer); shared by all the pictures of the document and read by
        /// `App::document_truncated`, so the title says so like it does for a reader's budget.
        render_truncated: Arc<std::sync::atomic::AtomicBool>,
        /// The longest side, in px, this picture can be drawn at inside the drawing process's work
        /// budget, learned by the decode thread after it drew; the wish for a sharper redraw is
        /// cut to it, so a heavy slide on a huge terminal is not redrawn again and again for a
        /// size it cannot have.
        raster_cap: Arc<SlideRasterCap>,
    },
}

/// How long a raster size refused for taking too long stays out of reach. A time-out measures the
/// machine's load as much as the slide, so it is not a fact about the slide: after this long a
/// revisit tries the sharp size again (a revisit within it does not wait out another time-out).
/// Refusals for cost or memory are the same on every run and never expire.
pub(super) const TIMEOUT_CAP_LIFETIME: std::time::Duration = std::time::Duration::from_secs(60);

/// The longest raster side, in px, a slide is known to be drawable at, shared by everything that
/// draws the slide. Two kinds of knowledge with different lifetimes: what the slide's own cost
/// allows and what the drawing process refused for cost or memory (the same every time, kept for
/// good), and what it refused only by the clock (expires, see [`TIMEOUT_CAP_LIFETIME`]).
#[derive(Debug, Default)]
pub(super) struct SlideRasterCap {
    /// Permanent limit; 0 = none known yet.
    permanent: std::sync::atomic::AtomicU32,
    /// A limit that came from a time-out, with when it was learned.
    timed_out: std::sync::Mutex<Option<(u32, std::time::Instant)>>,
}

impl SlideRasterCap {
    /// The limit in force at `now`; None = nothing known (draw at the wanted size).
    pub(super) fn get(&self, now: std::time::Instant) -> Option<u32> {
        let permanent =
            Some(self.permanent.load(std::sync::atomic::Ordering::Relaxed)).filter(|c| *c > 0);
        let timed_out = self
            .timed_out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .filter(|(_, at)| now.saturating_duration_since(*at) < TIMEOUT_CAP_LIFETIME)
            .map(|(px, _)| px);
        match (permanent, timed_out) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Lowers the permanent limit to `px` (never raises it; at least 1).
    pub(super) fn lower_permanent(&self, px: u32) {
        use std::sync::atomic::Ordering::Relaxed;
        let px = px.max(1);
        let mut current = self.permanent.load(Relaxed);
        // compare_exchange_weak loop (fetch_update is deprecated on 1.99, try_update needs
        // newer than the MSRV): stop as soon as the stored limit is already lower.
        while current == 0 || px < current {
            match self
                .permanent
                .compare_exchange_weak(current, px, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(seen) => current = seen,
            }
        }
    }

    /// Lowers the limit to `px` for [`TIMEOUT_CAP_LIFETIME`] from `now`. A limit that is still
    /// in force and lower stays (its own clock keeps running); an expired one is replaced.
    pub(super) fn lower_timed_out(&self, px: u32, now: std::time::Instant) {
        let px = px.max(1);
        let mut slot = self
            .timed_out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let live = slot.filter(|(_, at)| now.saturating_duration_since(*at) < TIMEOUT_CAP_LIFETIME);
        if live.is_none_or(|(old, _)| px < old) {
            *slot = Some((px, now));
        }
    }
}

/// One picture of the open document, held in memory only.
pub struct DocPicture {
    pub(super) source: PictureSource,
    /// Pixel size read from the header (raster) or the intrinsic size (SVG, slide), without
    /// decoding. `None` = not an image konoma can size: it is drawn as its alt text.
    pub(super) dims: Option<(u32, u32)>,
    /// `dims` came from a raster header (not an SVG's intrinsic size): only those are refused
    /// from the header alone; an SVG is guarded by its drawing process instead.
    pub(super) raster: bool,
}

/// A document as a plugin returned it (a Markdown file, a converted Word document or a
/// presentation): the host's container for the one on screen.
pub struct LoadedDocument {
    /// The Markdown the view draws: the file's text, the converted text, or a presentation's
    /// picture view (the same headings with the slide pictures between them), as the
    /// representation on screen says.
    pub(super) markdown: String,
    /// The conversion stopped at one of its budgets (the end, some pictures, or part of a table is
    /// missing).
    pub(super) truncated: bool,
    /// A slide was drawn with something left out (see `PictureSource::Lazy`).
    pub(super) render_truncated: Arc<std::sync::atomic::AtomicBool>,
    /// Pictures by their `office-img://...` URL (the slides' pictures included).
    pub(super) pictures: HashMap<String, DocPicture>,
    /// The representation `markdown` is (an id of the plugin's `Layout.reprs`); `None` for a
    /// document with a single one. Set when the view lands.
    pub(super) repr: Option<String>,
    /// How the document is drawn (the plugin's `DocumentView::options`): the document widget reads
    /// these, not the `[ui] md_*` settings.
    pub(super) options: DocOptions,
    /// Where the text came from: a file (the checkboxes can be written back) or a conversion.
    pub(super) origin: Origin,
}

/// Pixel size of an in-memory picture without decoding it: raster formats first, then SVG (the same
/// order as `md_image_dims` for a file).
fn picture_dims(bytes: &[u8]) -> (Option<(u32, u32)>, bool) {
    let raster = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok());
    match raster {
        Some(d) => (Some(d), true),
        None => (crate::preview::svg::intrinsic_size_bytes(bytes), false),
    }
}

impl LoadedDocument {
    /// Builds the App-side form on the worker thread (sizes are read here, so the UI thread never
    /// parses an image header).
    #[cfg(test)]
    pub(super) fn from_parts(markdown: String, truncated: bool, resources: Resources) -> Self {
        Self::from_view(
            markdown,
            truncated,
            resources,
            DocOptions {
                front_matter: false,
                tasks_writable: false,
                ..DocOptions::default()
            },
            Origin::Converted {
                from: PathBuf::new(),
            },
        )
    }

    /// The container for a plugin's document view.
    pub(super) fn from_view(
        markdown: String,
        truncated: bool,
        resources: Resources,
        options: DocOptions,
        origin: Origin,
    ) -> Self {
        let render_truncated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut pictures: HashMap<String, DocPicture> = HashMap::new();
        for (key, resource) in resources.iter() {
            let picture = match resource {
                Resource::Bytes(bytes) => {
                    let (dims, raster) = picture_dims(bytes);
                    DocPicture {
                        source: PictureSource::Bytes(bytes.clone()),
                        dims,
                        raster,
                    }
                }
                Resource::Lazy(lazy) => DocPicture {
                    dims: lazy.natural_size(),
                    source: PictureSource::Lazy {
                        lazy: lazy.clone(),
                        render_truncated: render_truncated.clone(),
                        raster_cap: Arc::new(SlideRasterCap::default()),
                    },
                    raster: false,
                },
            };
            pictures.insert(key.clone(), picture);
        }
        LoadedDocument {
            markdown,
            truncated,
            render_truncated,
            pictures,
            repr: None,
            options,
            origin,
        }
    }

    /// Whether the text was converted from another file (not read from a Markdown file).
    pub(super) fn is_converted(&self) -> bool {
        matches!(self.origin, Origin::Converted { .. })
    }
}

/// Test-only: the pixel size of a slide drawn from `scene`.
#[cfg(test)]
pub(super) use konoma_office::slide_px;

/// Test-only: a slide's picture source over a blank scene, with `cap` as its raster cap.
#[cfg(test)]
pub(super) fn stub_slide_source(cap: Arc<SlideRasterCap>) -> PictureSource {
    lazy_slide_source(Default::default(), cap)
}

/// Test-only: a slide's picture source drawn from `scene` (nothing in its media), with `cap` as
/// its raster cap.
#[cfg(test)]
pub(super) fn lazy_slide_source(
    scene: crate::preview::office::slide_draw::SlideScene,
    cap: Arc<SlideRasterCap>,
) -> PictureSource {
    PictureSource::Lazy {
        lazy: konoma_office::slide_resource(scene, HashMap::new()),
        render_truncated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        raster_cap: cap,
    }
}

/// Whether `path` is a presentation by its extension (PowerPoint or OpenDocument; the old binary
/// `.ppt` included, which is told apart as unreadable). Decides which plugin a file under the old
/// `builtin = "document"` rule goes to.
pub(crate) fn is_presentation_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" | "ppt" | "odp" | "otp"
            )
        })
}

/// The slide the view is on: the last heading at or above the top. `None` before the first
/// heading. `heads` are the headings' positions in the unit `top` uses. (Every heading can reach
/// the top: a presentation's scroll limit is widened to the last heading, `App::slide_scroll_limit`.)
pub(super) fn slide_at(heads: &[usize], top: usize) -> Option<usize> {
    heads.iter().rposition(|h| *h <= top)
}

/// Where `J` (`dir > 0`) / `K` (`dir < 0`) goes, as the index of a slide's heading. `J`: the slide
/// after the one the view is on. `K`: the last heading *above* the top of the view - the start of
/// the current slide when the view is inside it, else the previous slide's. `None` = nowhere to go.
pub(super) fn slide_turn_target(heads: &[usize], top: usize, dir: i32) -> Option<usize> {
    if dir >= 0 {
        let next = slide_at(heads, top).map_or(0, |c| c + 1);
        return (next < heads.len()).then_some(next);
    }
    heads.iter().rposition(|h| *h < top)
}

impl App {
    /// Replaces the open Word document. **The one place a document is dropped**: its pictures can
    /// total ~100 MiB, so the old one is freed off the UI thread (`discard_in_background`).
    pub(super) fn set_document(&mut self, doc: Option<Box<LoadedDocument>>) {
        if let Some(old) = std::mem::replace(&mut self.document, doc) {
            super::table_actions::discard_in_background(old);
        }
    }

    /// Whether the preview is a Word document or a presentation (loaded or not): the file the
    /// `document` or `presentation` plugin shows. (A *converted* document: it has no file with
    /// the lines it shows, and is never written back.)
    pub fn is_document(&self) -> bool {
        self.tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_office_document)
    }

    /// Whether the preview is drawn by the host's document widget from a plugin's document view
    /// (loaded or not): a Markdown file, a Word document or a presentation. The host's container
    /// for it is [`App::document`](App).
    pub fn holds_document(&self) -> bool {
        self.tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_document_kind)
    }

    /// Whether the document is here and can be drawn.
    pub fn document_ready(&self) -> bool {
        self.holds_document() && self.document.is_some()
    }

    /// A document whose text is not on screen: its plugin is still reading it (also when the tab
    /// came back in the source view) or failed. The one predicate the footer, the `?` help and the
    /// text keys (`/`, `v`, `V`) share, so what is offered is what acts
    /// ([[hint-shown-iff-key-acts]]). A *re*load keeps the old text and is not "missing".
    pub fn document_text_missing(&self) -> bool {
        self.holds_document() && !self.document_ready()
    }

    /// True while the plugin has not delivered the document yet (the "loading" screen). A *re*load
    /// keeps the previous text on screen instead.
    pub fn is_document_loading(&self) -> bool {
        self.holds_document()
            && self.document.is_none()
            && self
                .plugin
                .as_ref()
                .is_some_and(|p| matches!(p.phase, super::plugin_host::PluginPhase::Loading))
    }

    /// Why the document could not be shown (`None` while loading or on success).
    pub fn document_error(&self) -> Option<&konoma_plugin::Failure> {
        if self.holds_document() {
            self.plugin_failure()
        } else {
            None
        }
    }

    /// Whether a converted document stopped at a budget, or drawing a slide had to leave something
    /// out (the title says so). A Markdown file cut at the preview limit says so inside its text.
    pub fn document_truncated(&self) -> bool {
        self.document.as_ref().is_some_and(|d| {
            d.is_converted()
                && (d.truncated
                    || d.render_truncated
                        .load(std::sync::atomic::Ordering::Relaxed))
        })
    }

    /// Test-only: the Markdown the decorated view draws, when there is one for the document on
    /// screen: the file's text, a presentation's picture view or text view, or the converted text.
    #[cfg(test)]
    pub fn document_markdown_for_test(&self) -> Option<&str> {
        if !self.holds_document() {
            return None;
        }
        Some(self.document.as_ref()?.markdown.as_str())
    }

    /// Test-only: the state of a plugin whose job produced neither a document nor a reason: the
    /// show failed with the plugin's damaged-file reason.
    #[cfg(test)]
    pub fn forget_document_error_for_test(&mut self) {
        if let Some(show) = self.plugin.as_mut() {
            show.phase = super::plugin_host::PluginPhase::Failed(konoma_office::doc_failure(
                &crate::preview::office::OfficeError::Corrupt("x".into()),
                konoma_office::DocKind::Word,
            ));
        }
    }

    /// Test-only: the plugin's layout says the document has `count` pages (a document whose
    /// headings are not exactly its slides).
    #[cfg(test)]
    pub fn set_anchored_count_for_test(&mut self, count: usize) {
        if let Some(show) = self.plugin.as_mut() {
            show.layout.pages = Some(PageLayout::Anchored {
                heading_level: konoma_office::SLIDE_HEADING_LEVEL,
                count,
            });
        }
    }

    /// Test-only: makes the open presentation stop being one in the host's eyes (its plugin
    /// reports no anchored pages), the state of a document whose headings are not its slides.
    #[cfg(test)]
    pub fn forget_anchored_pages_for_test(&mut self) {
        if let Some(show) = self.plugin.as_mut() {
            show.layout.pages = None;
        }
    }

    /// Test-only: the temp file the raw (`R`) view reads, while it exists.
    #[cfg(test)]
    pub fn document_raw_file_for_test(&self) -> Option<PathBuf> {
        self.windowed_src().map(Path::to_path_buf)
    }

    /// Test-only: how many pictures the open document holds.
    #[cfg(test)]
    pub fn document_picture_count_for_test(&self) -> usize {
        self.document.as_ref().map_or(0, |d| d.pictures.len())
    }

    /// Test-only: the windowed reader's top byte offset.
    #[cfg(test)]
    pub fn preview_byte_top_for_test(&self) -> u64 {
        self.tab.preview_byte_top
    }

    /// Test-only: the url of the first picture of the open document.
    #[cfg(test)]
    pub fn document_first_picture_url_for_test(&self) -> Option<String> {
        let mut urls: Vec<_> = self.document.as_ref()?.pictures.keys().cloned().collect();
        urls.sort();
        urls.into_iter().next()
    }

    /// Test-only: pretends `n` more picture decodes are in flight (placeholders that never land).
    #[cfg(test)]
    pub fn fake_office_decodes_in_flight_for_test(&mut self, n: usize) {
        for i in 0..n {
            self.md_image_cache.insert(
                PathBuf::from(format!("office-img://fake-{i}")),
                MdImgEntry::default(),
            );
        }
    }

    /// Test-only: drops the placeholders `fake_office_decodes_in_flight_for_test` made.
    #[cfg(test)]
    pub fn clear_fake_office_decodes_for_test(&mut self) {
        self.md_image_cache
            .retain(|k, _| !k.to_string_lossy().starts_with("office-img://fake-"));
    }

    /// Test-only: forgets the cache entry of picture `url`.
    #[cfg(test)]
    pub fn forget_office_picture_for_test(&mut self, url: &str) {
        self.md_image_cache.remove(&PathBuf::from(url));
    }

    /// Test-only: whether the picture `url` has a cache entry (its decode was started).
    #[cfg(test)]
    pub fn office_picture_started_for_test(&self, url: &str) -> bool {
        self.md_image_cache.contains_key(&PathBuf::from(url))
    }

    /// Test-only: the size of the decoded pixels of picture `url`, `None` while there are none.
    #[cfg(test)]
    pub fn office_picture_pixels_for_test(&self, url: &str) -> Option<(u32, u32)> {
        use image::GenericImageView;
        let e = self.md_image_cache.get(&PathBuf::from(url))?;
        e.decoded.as_ref().map(|d| d.dimensions())
    }

    /// Test-only: whether a sharper redraw of picture `url` is running.
    #[cfg(test)]
    pub fn office_picture_reraster_inflight_for_test(&self, url: &str) -> bool {
        self.md_image_cache
            .get(&PathBuf::from(url))
            .is_some_and(|e| e.reraster_inflight)
    }

    /// Test-only: the RGBA of the decoded picture `url` at the fraction `(fx, fy)` of its size.
    #[cfg(test)]
    pub fn office_picture_rgba_for_test(&self, url: &str, fx: f64, fy: f64) -> Option<[u8; 4]> {
        use image::GenericImageView;
        let e = self.md_image_cache.get(&PathBuf::from(url))?;
        let d = e.decoded.as_ref()?;
        let (w, h) = d.dimensions();
        let x = ((w - 1) as f64 * fx).round() as u32;
        let y = ((h - 1) as f64 * fy).round() as u32;
        Some(d.get_pixel(x, y).0)
    }

    /// Test-only: why picture `url` cannot be shown, if it cannot.
    #[cfg(test)]
    pub fn office_picture_failure_for_test(
        &self,
        url: &str,
    ) -> Option<crate::preview::image::ImageFailure> {
        self.md_image_cache.get(&PathBuf::from(url))?.fail
    }

    /// Test-only: makes the next layout pass rebuild the page from scratch.
    #[cfg(test)]
    pub fn invalidate_md_cache_for_test(&mut self) {
        self.md_cache = None;
    }

    /// Test-only: makes the preview "move on" for any decode already started or started from now
    /// on (the media generation every decode thread compares against changes under it).
    #[cfg(test)]
    pub fn make_running_decodes_stale_for_test(&mut self) {
        self.media_gen_shared.store(
            self.media_gen.wrapping_add(1000),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Test-only: the inline-image cache's own budget (bytes), applied on every landed decode.
    #[cfg(test)]
    pub fn set_md_cache_budget_for_test(&mut self, budget: u64) {
        self.md_cache_budget_for_test = Some(budget);
    }

    /// Test-only: how many pictures of the document the cache holds pixels for right now.
    #[cfg(test)]
    pub fn office_pictures_with_pixels_for_test(&self) -> usize {
        self.md_image_cache
            .iter()
            .filter(|(k, e)| {
                crate::preview::markdown::is_office_image_url(&k.to_string_lossy())
                    && e.decoded.is_some()
            })
            .count()
    }

    /// Test-only: how many pictures of the document were drawn once and had their pixels dropped
    /// by the cache (they wait for a rebuild; no decode runs for them).
    #[cfg(test)]
    pub fn office_pictures_evicted_for_test(&self) -> usize {
        self.md_image_cache
            .iter()
            .filter(|(k, e)| {
                crate::preview::markdown::is_office_image_url(&k.to_string_lossy())
                    && e.decoded.is_none()
                    && e.evicted
            })
            .count()
    }

    /// Test-only: pictures of the document being decoded for the first time right now.
    #[cfg(test)]
    pub fn office_pictures_in_flight_for_test(&self) -> usize {
        self.office_pictures_in_flight()
    }

    /// Test-only: a weak handle on the bytes of picture `url` (alive while the document, or a
    /// decode of it, still holds them).
    #[cfg(test)]
    pub fn document_picture_weak_for_test(&self, url: &str) -> Option<std::sync::Weak<[u8]>> {
        match self.document_picture_source(url)? {
            PictureSource::Bytes(b) => Some(Arc::downgrade(&b)),
            PictureSource::Lazy { .. } => None,
        }
    }

    /// Test-only: the inline-image cache's eviction with a budget of `budget` bytes.
    #[cfg(test)]
    pub fn evict_md_images_to_for_test(&mut self, budget: u64) {
        self.evict_md_images_to(budget);
    }

    /// Pixel size of one picture of the open document.
    pub(super) fn document_picture_dims(&self, url: &str) -> Option<(u32, u32)> {
        self.document.as_ref()?.pictures.get(url)?.dims
    }

    /// Whether the size of picture `url` was read from a raster header (an SVG's is not).
    pub(super) fn document_picture_is_raster(&self, url: &str) -> bool {
        self.document
            .as_ref()
            .and_then(|d| d.pictures.get(url))
            .is_some_and(|p| p.raster)
    }

    /// Where the pixels of one picture of the open document come from (shared, not copied).
    pub(super) fn document_picture_source(&self, url: &str) -> Option<PictureSource> {
        Some(self.document.as_ref()?.pictures.get(url)?.source.clone())
    }

    /// The cap on the raster size of slide picture `url` found when it was last drawn (see
    /// `PictureSource::Lazy::raster_cap`); `None` for anything else or before the first draw.
    pub(super) fn slide_raster_cap(&self, url: &str) -> Option<u32> {
        match &self.document.as_ref()?.pictures.get(url)?.source {
            PictureSource::Lazy { raster_cap, .. } => raster_cap.get(std::time::Instant::now()),
            PictureSource::Bytes(_) => None,
        }
    }

    /// Whether picture `url` is made on demand (the drawing of a slide: sized and drawn by the
    /// slide rules).
    pub(super) fn document_picture_is_slide(&self, url: &str) -> bool {
        self.document
            .as_ref()
            .and_then(|d| d.pictures.get(url))
            .is_some_and(|p| matches!(p.source, PictureSource::Lazy { .. }))
    }

    /// What arrived from the plugin for a Markdown file, a Word document or a presentation.
    /// Dropped (off the UI thread) if the preview has since moved to something else. `asked` is
    /// the representation the request named (`None` = the plugin's default for the terminal).
    pub(super) fn land_document(&mut self, mut doc: Box<LoadedDocument>, asked: Option<&str>) {
        if !self.holds_document() {
            super::table_actions::discard_in_background(doc);
            return;
        }
        let layout = self.plugin.as_ref().map(|p| p.layout.clone());
        doc.repr = layout.as_ref().and_then(|l| self.effective_repr(l, asked));
        // A switch of representation (`R`) keeps the page (the slide) the view was on: read it
        // from the layout of the view being left, before anything of it is reset.
        let switching = self
            .document
            .as_ref()
            .is_some_and(|old| old.repr != doc.repr);
        let keep = switching.then(|| {
            self.anchored_position()
                .map(|(n, _)| n - 1)
                .or(self.tab.deck_pending_slide)
        });
        // Pictures are filed under a hash of their bytes, so a reload cannot leave a *wrong* entry
        // behind, only unreferenced ones: reclaim those.
        self.md_image_cache.retain(|k, _| {
            let s = k.to_string_lossy();
            !crate::preview::markdown::is_office_image_url(&s) || doc.pictures.contains_key(&*s)
        });
        self.set_document(Some(doc));
        self.md_cache = None;
        if let Some(slide) = keep {
            // The new view starts at the top and is laid out by the next draw (its width is the
            // draw's); that draw puts the page's heading at the top (`apply_pending_slide`).
            self.tab.preview_scroll = 0;
            self.tab.preview_hscroll = 0;
            self.tab.focused_item = None;
            self.md_items.clear();
            self.tab.deck_pending_slide = slide;
        }
        // A search in force ran on the old text; its hits are re-scanned lazily by the next `n`.
        if self.is_md_raw() {
            // Source view: the reader holds the old text (a converted document's temp file, or the
            // file as it was). Ask the session for the source again and reopen the reader.
            self.show_source_view();
        }
    }

    /// The plugin could not show the document (the reason is the show's failure): nothing of an
    /// earlier document stays on screen.
    pub(super) fn document_failed(&mut self) {
        if !self.holds_document() {
            return;
        }
        self.set_document(None);
        self.md_cache = None;
        // A raw (`R`) view of the previous text must not outlive the document it came from: drop
        // the temp file and the reader so the reason is what is shown.
        self.clear_command_out();
        if self.is_md_raw() {
            self.tab.repr = None;
        }
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.preview_visual_anchor = None;
    }

    /// The representation a view answered: the one asked for, else the layout's default for this
    /// terminal.
    fn effective_repr(&self, layout: &Layout, asked: Option<&str>) -> Option<String> {
        asked.map(str::to_string).or_else(|| {
            if self.terminal_draws_pixels() {
                layout.default_repr.clone()
            } else {
                layout
                    .default_repr_without_pixels
                    .clone()
                    .or_else(|| layout.default_repr.clone())
            }
        })
    }

    /// Test-only: a Word document or presentation put straight into the preview: the preview
    /// kind is set to the plugin that shows `path`'s extension, and the converted `doc` goes
    /// through the same road as a plugin's answer (a session over it, the host's container built
    /// from its view).
    #[cfg(test)]
    pub fn show_document_for_test(
        &mut self,
        path: PathBuf,
        doc: crate::preview::office::docx::Document,
    ) {
        self.tab.preview_kind = Some(PreviewKind::office_document(path.clone()));
        self.tab.preview_path = Some(path);
        self.land_document_for_test(doc);
    }

    /// Test-only: a converted presentation put straight into the open document: the same road a
    /// plugin's answer takes (`PluginJob::View` over a session made from `doc`).
    #[cfg(test)]
    pub fn land_document_for_test(&mut self, doc: crate::preview::office::docx::Document) {
        use super::plugin_host::{PluginJob, PluginPhase, PluginShow};
        let Some(PreviewKind::Plugin { id, path, .. }) = self.tab.preview_kind.clone() else {
            return;
        };
        let kind = if id == konoma_office::PRESENTATION_ID {
            konoma_office::DocKind::Deck
        } else {
            konoma_office::DocKind::Word
        };
        let session = konoma_office::OfficeSession::from_converted(
            kind,
            path.clone(),
            konoma_office::Converted::from_document(doc),
        );
        if !self
            .plugin
            .as_ref()
            .is_some_and(|p| p.id == id && p.path == path)
        {
            self.drop_plugin();
            self.plugin = Some(PluginShow {
                id: id.clone(),
                path: path.clone(),
                phase: PluginPhase::Loading,
                session: None,
                layout: Layout::single(),
                view: None,
                req: konoma_plugin::ViewReq::default(),
            });
        }
        let mut work = self.plugin_work(
            id,
            path,
            PluginJob::View {
                session: Box::new(session),
            },
            self.plugin_view_req(),
        );
        work.gen = self.media_gen;
        let result = work.run_on_the_spot();
        self.install_plugin_result(result);
    }

    // --- The pages of a presentation (`J`/`K`, the chip, the hints) ----------------------------

    /// The anchored pages of the open document: `(heading level, page count)` when its plugin
    /// reports one long document whose pages start at headings (the slides of a presentation),
    /// and the document is on screen.
    fn anchored_pages(&self) -> Option<(u8, usize)> {
        if !self.document_ready() {
            return None;
        }
        match self.plugin.as_ref()?.layout.pages {
            Some(PageLayout::Anchored {
                heading_level,
                count,
            }) if count > 0 => Some((heading_level, count)),
            _ => None,
        }
    }

    /// Logical lines of the pages' headings in the decorated view, or `None` when they are not
    /// exactly the open document's pages (then nothing is offered).
    fn anchored_head_lines(&self, c: &MdCache) -> Option<Vec<usize>> {
        let (level, count) = self.anchored_pages()?;
        let heads: Vec<usize> = c
            .anchors
            .iter()
            .filter(|(_, line)| {
                c.lines.get(*line).is_some_and(|l| {
                    crate::preview::markdown::heading_level_hint(
                        l,
                        crate::preview::markdown::row_after_heading(&c.lines, *line),
                    ) == level
                })
            })
            .map(|(_, line)| *line)
            .collect();
        (heads.len() == count).then_some(heads)
    }

    /// The pages' headings as the view shows them (display rows), with the view's top:
    /// `(heads, top)`. `None` when this is not a paged document on screen, or its headings are not
    /// exactly its pages.
    fn anchored_view(&self) -> Option<(Vec<usize>, usize)> {
        self.anchored_pages()?;
        let c = self.md_cache.as_ref()?;
        let heads: Vec<usize> = self
            .anchored_head_lines(c)?
            .into_iter()
            .map(|line| self.md_visual_span(line).0)
            .collect();
        Some((heads, self.tab.preview_scroll as usize))
    }

    /// The furthest the decorated view of a document may scroll: `total_rows - viewport` as for any
    /// text, but for a paged document at least the last page's heading row, so every page (the
    /// last ones too) can be put at the top and `J`/`K` stop on each. Below the last page is then
    /// blank, as below the last page of a PDF. The one limit `G`, `j`, `Space`, the outline, the
    /// search and a resize all end up clamped to (the draw path applies it).
    pub(crate) fn slide_scroll_limit(&self, total_rows: usize, viewport: usize) -> usize {
        let base = total_rows.saturating_sub(viewport);
        let last_head = self
            .md_cache
            .as_ref()
            .and_then(|c| self.anchored_head_lines(c))
            .and_then(|h| h.last().copied())
            .map(|line| self.md_visual_span(line).0);
        last_head.map_or(base, |h| base.max(h))
    }

    /// `(n, total)`: the page at the top of the view (1-based; hidden slides count) of an open
    /// paged document. The slide chip in the status line.
    pub fn anchored_position(&self) -> Option<(usize, usize)> {
        let (heads, top) = self.anchored_view()?;
        let cur = slide_at(&heads, top)?;
        Some((cur + 1, heads.len()))
    }

    /// [`App::anchored_position`] under its old name (the slides of a presentation).
    #[cfg(test)]
    pub fn slide_position(&self) -> Option<(usize, usize)> {
        self.anchored_position()
    }

    /// Whether `J`/`K` can turn between the pages of the document now: 2+ pages whose text is on
    /// screen. The one predicate the footer, the `?` help and `anchored_turn` share
    /// ([[hint-shown-iff-key-acts]]).
    pub fn anchored_can_turn(&self) -> bool {
        if self.tab.deck_pending_slide.is_some() {
            // The view is waiting to be laid out again: the page to keep is the position, and
            // the layout says how many pages there are.
            return self.anchored_pages().is_some_and(|(_, count)| count >= 2);
        }
        self.anchored_view()
            .is_some_and(|(heads, ..)| heads.len() >= 2)
    }

    /// [`App::anchored_can_turn`] under its old name (the slides of a presentation).
    #[cfg(test)]
    pub fn slide_can_turn(&self) -> bool {
        self.anchored_can_turn()
    }

    /// `J`/`K` on a paged document: scrolls the view to the next / previous page's heading.
    pub(super) fn anchored_turn(&mut self, dir: i32) {
        if !self.anchored_can_turn() {
            return;
        }
        // Keys of one batch are all handled before the next draw: while the view waits to be laid
        // out again (`R`, a resize) the page to keep is the position, so a `J`/`K` that arrives
        // first moves that page instead of being lost.
        if let Some(pending) = self.tab.deck_pending_slide {
            let last = self
                .anchored_pages()
                .map_or(0, |(_, count)| count)
                .saturating_sub(1);
            self.tab.deck_pending_slide = Some(if dir >= 0 {
                (pending + 1).min(last)
            } else {
                pending.saturating_sub(1)
            });
            return;
        }
        let Some((heads, top)) = self.anchored_view() else {
            return;
        };
        let Some(i) = slide_turn_target(&heads, top, dir) else {
            return;
        };
        self.tab.preview_scroll = heads[i].min(u16::MAX as usize) as u16;
    }

    // --- The representations of a document (slide pictures / text) -----------------------------

    /// Whether the open document is a presentation (loaded): its plugin reports anchored pages.
    pub fn is_deck(&self) -> bool {
        self.anchored_pages().is_some()
    }

    /// Whether the open presentation has a picture representation at all (its reader drew the
    /// slides).
    fn deck_has_pictures(&self) -> bool {
        self.is_deck()
            && self.plugin.as_ref().is_some_and(|p| {
                p.layout
                    .reprs
                    .iter()
                    .any(|r| r.id == konoma_office::REPR_PICTURE)
            })
    }

    /// Whether this terminal draws real pixels (kitty, iTerm2 or sixel) rather than half-block
    /// cells: the one question the default representation of a presentation depends on. A picture
    /// of a slide is unreadable in half blocks, and without an image backend nothing is drawn at
    /// all.
    pub(super) fn terminal_draws_pixels(&self) -> bool {
        self.picker.as_ref().is_some_and(|p| {
            !matches!(
                p.protocol_type(),
                ratatui_image::picker::ProtocolType::Halfblocks
            )
        })
    }

    /// Whether the presentation on screen is shown as the pictures of its slides: it has them, and
    /// the representation that landed is the picture one (the tab's choice `R`, else the plugin's
    /// default for the terminal). The one predicate the renderer, the keys and the hints share.
    pub fn deck_picture_view(&self) -> bool {
        self.deck_has_pictures()
            && self
                .document
                .as_ref()
                .is_some_and(|d| d.repr.as_deref() == Some(konoma_office::REPR_PICTURE))
    }

    /// What `R` does on a presentation, as the footer's label: `None` when it does nothing (the
    /// plugin offers one representation only). The `?` help's row and the handler use the same
    /// predicate.
    pub fn deck_view_hint(&self) -> Option<crate::i18n::Msg> {
        use crate::i18n::Msg;
        self.deck_has_pictures().then(|| {
            if self.deck_picture_view() {
                Msg::HintDeckText
            } else {
                Msg::HintDeckSlides
            }
        })
    }

    /// `deck_view_hint` for the `?` help.
    pub fn deck_view_help(&self) -> Option<crate::i18n::Msg> {
        use crate::i18n::Msg;
        self.deck_view_hint().map(|m| match m {
            Msg::HintDeckText => Msg::DeckTextHelp,
            _ => Msg::DeckSlidesHelp,
        })
    }

    /// `R` on a presentation: asks the plugin for the next representation (slide pictures / text),
    /// keeping the current slide (the one whose heading is at or above the top of the view; it is
    /// read when the new view lands, `land_document`). A no-op when the plugin offers one.
    pub(super) fn toggle_deck_view(&mut self) {
        if !self.deck_has_pictures() {
            return;
        }
        let next = if self.deck_picture_view() {
            konoma_office::REPR_TEXT
        } else {
            konoma_office::REPR_PICTURE
        };
        let before = self.tab.repr.replace(next.to_string());
        if !self.plugin_view_now(self.plugin_view_req()) {
            // The session is busy (a read is running): the key is ignored, the choice unchanged.
            self.tab.repr = before;
        }
    }

    /// Scrolls to the page `R` or a resize asked to keep, once the new view's cache exists
    /// (its rows are only known then). Called by the draw path right after the layout.
    pub(crate) fn apply_pending_slide(&mut self) {
        let Some(slide) = self.tab.deck_pending_slide else {
            return;
        };
        // No layout to read the headings from yet: keep the wish for the draw that has one.
        let Some((heads, _)) = self.anchored_view() else {
            return;
        };
        self.tab.deck_pending_slide = None;
        if let Some(h) = heads.get(slide) {
            self.tab.preview_scroll = (*h).min(u16::MAX as usize) as u16;
        }
    }

    /// Before a draw that lays the view out again at another size (a resize): remembers the page
    /// at the top, so `apply_pending_slide` puts it back at the top of the new layout. The scroll
    /// is a row number of the old layout and means another page in the new one. `width` and
    /// `slide_rows` are what the coming layout will use.
    pub(crate) fn keep_slide_across_relayout(&mut self, width: u16) {
        if self.tab.deck_pending_slide.is_some() || !self.deck_picture_view() {
            return;
        }
        let Some(c) = self.md_cache.as_ref() else {
            return;
        };
        if c.width == width && c.slide_rows == self.slide_fit_rows() {
            return;
        }
        self.tab.deck_pending_slide = self.anchored_position().map(|(n, _)| n - 1);
    }

    /// Test-only: the tab's own choice of representation (`Some("text")`), `None` = the default.
    #[cfg(test)]
    pub fn deck_view_choice_for_test(&self) -> Option<bool> {
        self.tab
            .repr
            .as_deref()
            .filter(|r| *r == konoma_office::REPR_TEXT || *r == konoma_office::REPR_PICTURE)
            .map(|r| r == konoma_office::REPR_TEXT)
    }

    /// Test-only: the URL of slide `n`'s (1-based) picture, if the open deck has pictures.
    #[cfg(test)]
    pub fn deck_slide_url_for_test(&self, n: usize) -> Option<String> {
        let suffix = format!("/slide-{n}.svg");
        let d = self.document.as_ref()?;
        d.pictures
            .iter()
            .find(|(k, p)| k.ends_with(&suffix) && matches!(p.source, PictureSource::Lazy { .. }))
            .map(|(k, _)| k.clone())
    }
}

#[cfg(test)]
impl MdImageResult {
    /// Test-only: the failure code the result carries (`None` for a picture).
    pub fn error_code_for_test(&self) -> Option<&str> {
        self.image.as_ref().err().map(String::as_str)
    }
}

#[cfg(test)]
mod slide_tests {
    use super::*;

    const H: [usize; 4] = [0, 10, 20, 30];

    #[test]
    fn the_slide_is_the_last_heading_at_or_above_the_top() {
        assert_eq!(slide_at(&H, 0), Some(0));
        assert_eq!(slide_at(&H, 9), Some(0));
        assert_eq!(slide_at(&H, 10), Some(1));
        assert_eq!(slide_at(&H, 30), Some(3));
        assert_eq!(slide_at(&H, 99), Some(3));
        assert_eq!(slide_at(&[], 0), None);
        // Text that starts before its first heading: no slide yet.
        assert_eq!(slide_at(&[3, 9], 1), None);
    }

    #[test]
    fn j_goes_to_the_next_heading_and_stops_after_the_last() {
        assert_eq!(slide_turn_target(&H, 0, 1), Some(1));
        assert_eq!(slide_turn_target(&H, 15, 1), Some(2));
        assert_eq!(slide_turn_target(&H, 20, 1), Some(3));
        assert_eq!(slide_turn_target(&H, 30, 1), None);
        assert_eq!(slide_turn_target(&[3, 9], 1, 1), Some(0));
    }

    #[test]
    fn k_goes_to_the_last_heading_above_the_top() {
        assert_eq!(slide_turn_target(&H, 15, -1), Some(1), "inside: its start");
        assert_eq!(
            slide_turn_target(&H, 10, -1),
            Some(0),
            "at a heading: the previous"
        );
        assert_eq!(
            slide_turn_target(&H, 30, -1),
            Some(2),
            "from the last slide"
        );
        assert_eq!(slide_turn_target(&H, 0, -1), None);
        assert_eq!(slide_turn_target(&[3, 9], 1, -1), None);
    }

    #[test]
    fn every_heading_is_stopped_on_in_both_directions() {
        let mut top = 0;
        let mut seen = vec![0];
        while let Some(i) = slide_turn_target(&H, top, 1) {
            top = H[i];
            seen.push(i);
        }
        assert_eq!(seen, [0, 1, 2, 3]);
        let mut back = vec![3];
        while let Some(i) = slide_turn_target(&H, top, -1) {
            top = H[i];
            back.push(i);
        }
        assert_eq!(back, [3, 2, 1, 0]);
    }
}

/// The state of a presentation without the worker: a converted deck put straight into an `App`.
/// Pins what `slide_view` / `slide_scroll_limit` and the key handlers do with exact numbers,
/// including the states the keys cannot reach one by one (a Markdown whose headings are not its
/// slides).
#[cfg(test)]
mod slide_state_tests {
    use super::*;
    use crate::preview::office::docx::pptx::SlideInfo;
    use crate::preview::office::docx::Document;

    /// Holds the `App` and its temp dir.
    struct Rig {
        app: App,
        _dir: crate::test_support::TmpDir,
    }

    fn slides(n: usize) -> Vec<SlideInfo> {
        (1..=n)
            .map(|number| SlideInfo {
                number,
                title: format!("T{number}"),
                hidden: false,
            })
            .collect()
    }

    /// `n` slides of 20 lines each: line `20 * k` is `## Slide k+1`, the rest are 19 body lines.
    fn deck_markdown(n: usize) -> String {
        let mut md = String::new();
        for k in 0..n {
            md.push_str(&format!("## Slide {}: T{}\n", k + 1, k + 1));
            for j in 1..20 {
                md.push_str(&format!("body {k}.{j}\n"));
            }
        }
        md
    }

    /// An `App` that holds a deck of `n` slides (no layout built yet).
    fn deck_rig(n: usize, vh: u16) -> Rig {
        let dir = crate::test_support::unique_tmp("slide_state");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.as_path().to_path_buf(), Config::default()).unwrap();
        let path = dir.as_path().join("d.pptx");
        app.show_document_for_test(
            path,
            Document {
                markdown: deck_markdown(n),
                slides: slides(n),
                ..Document::default()
            },
        );
        app.tab.preview_viewport = vh;
        Rig { app, _dir: dir }
    }

    // --- the slide the view is on, and J / K ------------------------------------------------

    #[test]
    fn a_one_slide_deck_has_a_position_but_no_turning() {
        let mut r = deck_rig(1, 30);
        r.app.ensure_md_cache(60);
        assert_eq!(r.app.slide_position(), Some((1, 1)));
        assert!(!r.app.slide_can_turn());
        r.app.anchored_turn(1);
        r.app.anchored_turn(-1);
        assert_eq!(r.app.tab.preview_scroll, 0);
    }

    #[test]
    fn a_two_slide_deck_can_turn() {
        let mut r = deck_rig(2, 30);
        r.app.ensure_md_cache(60);
        assert!(r.app.slide_can_turn());
        assert_eq!(r.app.slide_position(), Some((1, 2)));
    }

    #[test]
    fn a_deck_is_never_in_the_raw_view_so_the_text_footer_has_no_slide_keys() {
        // `R` on a deck switches pictures / text, never to the raw source: the footer's code/text
        // branch (the only one that could be reached in a raw view) therefore offers no `J/K`.
        let mut r = deck_rig(3, 30);
        r.app.ensure_md_cache(60);
        assert!(r.app.slide_can_turn());
        r.app.toggle_md_raw();
        r.app.toggle_md_raw();
        assert!(!r.app.is_raw_source());
        // And if a raw view did exist (windowed reader, no decorated cache), `J`/`K` would not act.
        r.app.tab.repr = Some("source".into());
        r.app.md_cache = None;
        assert!(!r.app.slide_can_turn());
    }

    #[test]
    fn j_and_k_do_nothing_where_the_keys_are_not_offered() {
        // One slide and some text before its heading: the view is before the first heading, so J
        // would have somewhere to go - but the key is not offered for a single slide.
        let mut r = deck_rig(1, 30);
        r.app.document.as_mut().unwrap().markdown = "intro\n\n## Slide 1: only\n\nbody\n".into();
        r.app.ensure_md_cache(60);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        r.app.anchored_turn(1);
        assert_eq!(r.app.tab.preview_scroll, 0);
        r.app.anchored_turn(-1);
        assert_eq!(r.app.tab.preview_scroll, 0);
        // Not a deck at all: nothing happens either.
        let mut w = deck_rig(3, 30);
        w.app.forget_anchored_pages_for_test();
        w.app.ensure_md_cache(60);
        w.app.anchored_turn(1);
        assert_eq!(w.app.tab.preview_scroll, 0);
    }

    #[test]
    fn no_document_shown_means_no_slides() {
        let dir = crate::test_support::unique_tmp("slide_none");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.as_path().to_path_buf(), Config::default()).unwrap();
        assert_eq!(app.slide_position(), None);
        assert!(!app.slide_can_turn());
        assert_eq!(app.slide_scroll_limit(100, 30), 70);
        app.anchored_turn(1);
        // A deck that is held but is not what the tab shows.
        let mut r = deck_rig(3, 30);
        r.app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("a.txt")));
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        assert_eq!(r.app.slide_scroll_limit(100, 30), 70);
    }

    #[test]
    fn the_decorated_limit_is_the_plain_one_without_a_layout() {
        // Decorated view of a deck whose layout is not built yet: nothing to widen.
        let r = deck_rig(3, 30);
        assert!(r.app.md_cache.is_none());
        assert_eq!(r.app.slide_scroll_limit(100, 30), 70);
        assert_eq!(r.app.slide_scroll_limit(10, 30), 0);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
    }

    // --- the turn targets ---------------------------------------------------------------------

    #[test]
    fn a_direction_of_zero_reads_as_forward() {
        // `J` is `dir > 0` ... and anything not negative is forward (nothing sends 0 today).
        assert_eq!(slide_turn_target(&[0, 10, 20], 0, 0), Some(1));
        assert_eq!(slide_turn_target(&[0, 10, 20], 20, 0), None);
    }

    #[test]
    fn turning_with_no_slides_goes_nowhere() {
        for dir in [-1, 0, 1] {
            assert_eq!(slide_turn_target(&[], 0, dir), None);
            assert_eq!(slide_turn_target(&[], 99, dir), None);
        }
        assert_eq!(slide_at(&[], 5), None);
    }

    #[test]
    fn one_heading_is_the_slide_everywhere_after_it() {
        assert_eq!(slide_at(&[4], 3), None);
        assert_eq!(slide_at(&[4], 4), Some(0));
        assert_eq!(slide_at(&[4], 400), Some(0));
        assert_eq!(slide_turn_target(&[4], 0, 1), Some(0));
        assert_eq!(slide_turn_target(&[4], 4, 1), None);
        assert_eq!(slide_turn_target(&[4], 4, -1), None);
        assert_eq!(slide_turn_target(&[4], 9, -1), Some(0));
    }

    // --- which files are presentations --------------------------------------------------------

    #[test]
    fn every_presentation_extension_is_one_in_any_case() {
        for ext in [
            "pptx", "pptm", "ppsx", "ppsm", "potx", "potm", "ppt", "odp", "otp",
        ] {
            for name in [
                format!("d.{ext}"),
                format!("d.{}", ext.to_uppercase()),
                format!("D.{}", {
                    let mut c = ext.chars();
                    c.next()
                        .unwrap()
                        .to_uppercase()
                        .chain(c)
                        .collect::<String>()
                }),
                format!("/some/dir/with.dots/deck.v2.{ext}"),
            ] {
                assert!(is_presentation_path(Path::new(&name)), "{name}");
            }
        }
    }

    #[test]
    fn nothing_else_is_a_presentation() {
        for name in [
            "d.docx",
            "d.docm",
            "d.dotx",
            "d.dotm",
            "d.odt",
            "d.ott",
            "d.doc",
            "d.xlsx",
            "d.ods",
            "d.pptxx",
            "d.ppt.txt",
            "d.pdf",
            "d.txt",
            "d.md",
            "d.zip",
            "pptx",
            "odp",
            ".pptx",
            "d.",
            "d",
            "",
        ] {
            assert!(!is_presentation_path(Path::new(name)), "{name}");
        }
    }

    // --- the decorated view ---------------------------------------------------------------------

    /// A deck in the decorated view (layout built for a `w`-column terminal), a `vh`-row viewport.
    /// Every slide's body is one long line that wraps over several rows at that width.
    fn decorated_rig(n: usize, w: u16, vh: u16) -> Rig {
        let mut md = String::new();
        for k in 0..n {
            md.push_str(&format!("## Slide {}: T{}\n\n", k + 1, k + 1));
            md.push_str(&format!("word{k} ").repeat(20 + 15 * k));
            md.push_str("\n\n");
        }
        let dir = crate::test_support::unique_tmp("slide_deco");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.as_path().to_path_buf(), Config::default()).unwrap();
        let path = dir.as_path().join("d.pptx");
        app.show_document_for_test(
            path,
            Document {
                markdown: md,
                slides: slides(n),
                ..Document::default()
            },
        );
        app.tab.preview_viewport = vh;
        app.ensure_md_cache(w);
        assert!(app.md_cache.is_some());
        Rig { app, _dir: dir }
    }

    /// The display row of each slide's heading, from the layout.
    fn head_rows(r: &Rig) -> Vec<usize> {
        let c = r.app.md_cache.as_ref().unwrap();
        r.app
            .anchored_head_lines(c)
            .unwrap()
            .into_iter()
            .map(|line| r.app.md_visual_span(line).0)
            .collect()
    }

    #[test]
    fn the_headings_are_found_by_display_row_not_by_line() {
        let r = decorated_rig(4, 40, 10);
        let rows = head_rows(&r);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], 0);
        let c = r.app.md_cache.as_ref().unwrap();
        let lines = r.app.anchored_head_lines(c).unwrap();
        // Wrapped text pushes every later heading further down than its line number.
        for k in 1..4 {
            assert!(
                rows[k] > lines[k] + k,
                "slide {k}: row {} line {}",
                rows[k],
                lines[k]
            );
            assert!(rows[k] > rows[k - 1]);
        }
        let (heads, top) = r.app.anchored_view().unwrap();
        assert_eq!(heads, rows);
        assert_eq!(top, 0);
    }

    #[test]
    fn j_and_k_in_the_decorated_view_scroll_to_the_heading_row() {
        let mut r = decorated_rig(4, 40, 10);
        let rows = head_rows(&r);
        for (k, row) in rows.iter().enumerate().skip(1) {
            r.app.anchored_turn(1);
            assert_eq!(r.app.tab.preview_scroll as usize, *row, "J to {k}");
            assert_eq!(r.app.slide_position(), Some((k + 1, 4)));
        }
        r.app.anchored_turn(1);
        assert_eq!(r.app.tab.preview_scroll as usize, rows[3], "the last stays");
        for k in (0..3).rev() {
            r.app.anchored_turn(-1);
            assert_eq!(r.app.tab.preview_scroll as usize, rows[k], "K to {k}");
        }
        r.app.anchored_turn(-1);
        assert_eq!(r.app.tab.preview_scroll, 0);
        // A top that is not on a heading: K goes to the start of the slide it is in.
        r.app.tab.preview_scroll = (rows[2] + 1) as u16;
        assert_eq!(r.app.slide_position(), Some((3, 4)));
        r.app.anchored_turn(-1);
        assert_eq!(r.app.tab.preview_scroll as usize, rows[2]);
        r.app.tab.preview_scroll = (rows[2] + 1) as u16;
        r.app.anchored_turn(1);
        assert_eq!(r.app.tab.preview_scroll as usize, rows[3]);
    }

    #[test]
    fn the_decorated_scroll_limit_reaches_the_last_heading() {
        let r = decorated_rig(4, 40, 10);
        let rows = head_rows(&r);
        let total = r
            .app
            .md_cache
            .as_ref()
            .unwrap()
            .row_prefix
            .last()
            .copied()
            .unwrap();
        // A viewport shorter than the last slide: the ordinary limit is further down.
        assert_eq!(r.app.slide_scroll_limit(total, 5), total - 5);
        // A viewport taller than the last slide: the last heading is the limit.
        let tall = total - rows[3] + 10;
        assert_eq!(r.app.slide_scroll_limit(total, tall), rows[3]);
        // Equal: either way.
        assert_eq!(r.app.slide_scroll_limit(total, total - rows[3]), rows[3]);
        // A viewport of 0 rows or a short document: never below the heading.
        assert_eq!(r.app.slide_scroll_limit(total, 0), total);
        assert_eq!(r.app.slide_scroll_limit(0, 10), rows[3]);
    }

    #[test]
    fn the_decorated_limit_is_the_ordinary_one_for_a_word_document() {
        let mut r = decorated_rig(4, 40, 10);
        // The same text as a Word document (no slides): no widening.
        r.app.forget_anchored_pages_for_test();
        let total = r
            .app
            .md_cache
            .as_ref()
            .unwrap()
            .row_prefix
            .last()
            .copied()
            .unwrap();
        assert_eq!(r.app.slide_scroll_limit(total, total + 50), 0);
        assert_eq!(r.app.slide_scroll_limit(total, 5), total - 5);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
    }

    #[test]
    fn headings_that_are_not_exactly_the_slides_offer_nothing() {
        // One slide more than headings, and one fewer.
        for delta in [1isize, -1] {
            let mut r = decorated_rig(4, 40, 10);
            r.app
                .set_anchored_count_for_test(if delta > 0 { 5 } else { 3 });
            assert_eq!(r.app.slide_position(), None, "delta {delta}");
            assert!(!r.app.slide_can_turn(), "delta {delta}");
            let total = r
                .app
                .md_cache
                .as_ref()
                .unwrap()
                .row_prefix
                .last()
                .copied()
                .unwrap();
            assert_eq!(
                r.app.slide_scroll_limit(total, total + 50),
                0,
                "delta {delta}"
            );
            r.app.anchored_turn(1);
            assert_eq!(r.app.tab.preview_scroll, 0, "delta {delta}");
        }
    }

    #[test]
    fn an_extra_level_two_heading_in_the_text_is_not_a_slide() {
        // The converter never writes one, but the count guard is what keeps the chip honest if it did.
        let mut r = decorated_rig(3, 40, 10);
        let mut md = r.app.document.as_ref().unwrap().markdown.clone();
        md.push_str("\n## An extra heading\n");
        r.app.document.as_mut().unwrap().markdown = md;
        r.app.md_cache = None;
        r.app.ensure_md_cache(40);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
    }

    #[test]
    fn a_heading_of_another_level_is_not_a_slide_heading() {
        // 3 slides and a level-1 and a level-3 heading among them: only the level-2 ones count.
        let mut r = decorated_rig(3, 40, 10);
        let mut md = String::from("# A title\n\n");
        md.push_str(&r.app.document.as_ref().unwrap().markdown);
        md.push_str("\n### A smaller one\n");
        r.app.document.as_mut().unwrap().markdown = md;
        r.app.md_cache = None;
        r.app.ensure_md_cache(40);
        assert_eq!(
            r.app.slide_position(),
            None,
            "before the first heading: no slide yet"
        );
        assert!(r.app.slide_can_turn());
        let (heads, _) = r.app.anchored_view().unwrap();
        assert_eq!(heads.len(), 3);
        r.app.anchored_turn(1);
        assert_eq!(r.app.slide_position(), Some((1, 3)));
        r.app.anchored_turn(1);
        assert_eq!(r.app.slide_position(), Some((2, 3)));
    }

    #[test]
    fn a_scroll_row_past_u16_is_capped_not_wrapped() {
        let mut r = decorated_rig(3, 40, 10);
        // Pretend every logical line is 40,000 rows tall.
        let lines = r.app.md_cache.as_ref().unwrap().lines.len();
        r.app.md_cache.as_mut().unwrap().row_prefix = (0..=lines).map(|i| i * 40_000).collect();
        let rows = head_rows(&r);
        assert!(rows[1] > u16::MAX as usize && rows[2] > rows[1]);
        r.app.anchored_turn(1);
        assert_eq!(r.app.tab.preview_scroll, u16::MAX);
    }

    #[test]
    fn the_decorated_view_without_wrapping_counts_lines() {
        let dir = crate::test_support::unique_tmp("slide_nowrap");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.wrap = false;
        let mut app = App::new(dir.as_path().to_path_buf(), cfg).unwrap();
        let path = dir.as_path().join("d.pptx");
        let md = deck_markdown(3);
        app.show_document_for_test(
            path,
            Document {
                markdown: md,
                slides: slides(3),
                ..Document::default()
            },
        );
        app.ensure_md_cache(60);
        let c = app.md_cache.as_ref().unwrap();
        let lines = app.anchored_head_lines(c).unwrap();
        let (heads, _) = app.anchored_view().unwrap();
        assert_eq!(heads, lines, "one row per line");
        app.anchored_turn(1);
        assert_eq!(app.tab.preview_scroll as usize, lines[1]);
    }

    #[test]
    fn a_deck_that_is_not_what_the_tab_shows_widens_nothing() {
        let mut r = decorated_rig(4, 40, 10);
        let total = r
            .app
            .md_cache
            .as_ref()
            .unwrap()
            .row_prefix
            .last()
            .copied()
            .unwrap();
        let tall = total + 50;
        let rows = head_rows(&r);
        assert_eq!(r.app.slide_scroll_limit(total, tall), rows[3]);
        r.app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("a.txt")));
        assert_eq!(r.app.slide_scroll_limit(total, tall), 0);
    }

    #[test]
    fn anchors_pointing_past_the_text_are_not_headings() {
        let mut r = decorated_rig(4, 40, 10);
        let n = r.app.md_cache.as_ref().unwrap().lines.len();
        r.app
            .md_cache
            .as_mut()
            .unwrap()
            .anchors
            .push(("ghost".into(), n + 5));
        assert_eq!(r.app.slide_position(), Some((1, 4)));
        assert!(r.app.slide_can_turn());
    }
}
