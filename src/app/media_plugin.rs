//! The picture plugins (`image`, `svg`, `pdf`, `video`) on the host side.
//!
//! A picture plugin's `view` returns an [`ImageView`]; the host draws it with the image pipeline it
//! always had (decoding under the size limits, GIF frames, the kitty compressed transfer, zoom and
//! pan, sharp zoom of vector sources). They go through the same road as every plugin
//! (`app/plugin_host.rs`): the plugin's work and the decoding run **in one go on the same
//! worker** ([`decode_image_view`] is the decoding step of that job), and the result is dropped on
//! arrival when it is no longer wanted, exactly like every other job. This module holds what is
//! particular to pictures: starting the job for a page, the decoding, and the reasons a failed
//! picture shows.
//!
//! Pages: the layout a plugin reports (`Layout.pages`) is the host's page state
//! (`PerTab::page`, `PerTab::page_count`), and `J`/`K` ask the session for another page while the
//! old picture stays on screen. The page count reaches the host as soon as `open` returns, before
//! the first page is drawn.

#[cfg(test)]
use konoma_plugin::view::ImageData;
use konoma_plugin::view::{ImageView, ViewReq};
use konoma_plugin::{Failure, FailureKind, Input, Plugin};

use super::plugin_host::{PluginJob, PluginPhase, PluginShow, Wait};
use super::*;

/// The reason an image failure shows for a plugin's `Failure`. The pictures that have a fixed
/// fallback text of their own (a PDF, a video) show that text, so they have no reason here.
///
/// What a failure is explained as follows from the plugin's declared `Unavailable`, not from its
/// id: a plugin whose file is still text when it cannot be drawn (`RawText { with_reason: true }`,
/// the SVG one) gets the vector-file reasons, one that names the host's picture frame
/// (`Picture`, the image one) gets the picture reasons, and any other (a PDF, a video, a plugin
/// the user installed) shows the failure's own message instead.
pub(super) fn image_failure_of(
    id: &str,
    f: &Failure,
) -> Option<crate::preview::image::ImageFailure> {
    use crate::preview::image::ImageFailure;
    use crate::preview::svg_guard::SvgFail;
    use konoma_plugin::Unavailable;
    match (crate::plugins::unavailable_of(id), f.kind) {
        (_, FailureKind::Cancelled) => Some(ImageFailure::Cancelled),
        (Unavailable::RawText { with_reason: true }, FailureKind::TooLarge) => {
            Some(ImageFailure::Svg(SvgFail::TooLarge))
        }
        (Unavailable::RawText { with_reason: true }, _) => {
            Some(ImageFailure::Svg(SvgFail::Invalid))
        }
        (Unavailable::Picture, FailureKind::TooLarge | FailureKind::TooHeavy) => {
            Some(ImageFailure::TooLarge)
        }
        (Unavailable::Picture, FailureKind::Unsupported) => Some(ImageFailure::UnsupportedFormat),
        (Unavailable::Picture, _) => Some(ImageFailure::Corrupt),
        _ => None,
    }
}

/// Konoma's drawer of the SVGs a plugin hands over that are not trusted: a supervised child
/// process of this executable (`preview::svg`).
pub(super) struct SvgChild;

impl konoma_plugin::canvas_pictures::SvgDraw for SvgChild {
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[std::path::PathBuf]>,
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, crate::preview::svg::RasterError> {
        crate::preview::svg::rasterize_untrusted_within(data, base, roots, max_px, cancelled)
    }

    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, crate::preview::svg::RasterError> {
        crate::preview::svg::rasterize_embedded(data, max_px, cancelled)
    }
}

/// Decode what a picture plugin returned into the host's media payload. `None` is a failure with
/// no reason to show (the render side falls back), as for any media job.
///
/// `trust_own_svg` is whether the host believes the plugin's `trusted` claim on an SVG it returns
/// (`plugins::draws_own_svg`: decided by where the plugin comes from). Without it every SVG is
/// drawn by the supervised child process; with no `base_dir` such an SVG reads no file.
pub(super) fn decode_image_view(
    iv: ImageView,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
    scope: &konoma_plugin::ReadScope,
) -> Option<MediaPayload> {
    // The decoding itself is shared with the standalone app (`decode_picture_view`); this host
    // draws an untrusted SVG in a supervised child (`SvgChild`) and keeps the result as a payload.
    konoma_plugin::canvas_pictures::decode_picture_view(
        iv,
        path,
        default_raster_px,
        &|| cancel.is_some_and(|c| c.is_cancelled()),
        trust_own_svg,
        scope,
        &SvgChild,
    )
    .map(payload_of)
}

/// What the shared decoding made, as the host's media payload.
pub(super) fn payload_of(view: konoma_plugin::canvas_pictures::PictureView) -> MediaPayload {
    use konoma_plugin::canvas_pictures::PictureView;
    match view {
        PictureView::Still(img) => MediaPayload::Static(img),
        PictureView::Vector { img, svg, trust } => MediaPayload::Vector { img, svg, trust },
        PictureView::Gif(frames) => MediaPayload::Gif(frames),
        PictureView::Failed(why) => MediaPayload::ImageFailed(why),
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only: a plugin that stands in for whatever the registry would give, on this thread.
    pub(crate) static STAND_IN: std::cell::RefCell<Option<Arc<dyn Plugin>>> =
        const { std::cell::RefCell::new(None) };
}

impl App {
    /// Start showing the file `path` with the picture plugin `id` for the current preview: on the
    /// worker (or on the spot without one), asking for the page the tab is on. The session of the
    /// page before is reused when it belongs to this file.
    pub(super) fn start_media_plugin(&mut self, id: &str, path: &Path) {
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        if !registry.is_enabled(id) {
            return;
        }
        let Some(plugin) = registry.plugin(id) else {
            return;
        };
        #[cfg(test)]
        let plugin = STAND_IN.with(|s| s.borrow().clone()).unwrap_or(plugin);
        // A still image needs the decode backend too (its encode is made asynchronously at render
        // time); every other picture only needs a graphics backend. An animation is a "GIF" to
        // the host when its first bytes say so.
        // Decided by the file's format (the judgment a media diff uses), not by the plugin's id; the
        // type the plugin returns is not known before it runs.
        let still = super::media_diff::media_format(path, None) == Some(MediaDiffKind::Image)
            && !Self::looks_like_gif(path);
        if self.picker.is_none() || (still && self.img_tx.is_none()) {
            return; // the render side falls back to text or a message
        }
        // A mermaid file is a picture only in image mode (`mode`, and an image backend, settled
        // above and here); otherwise the render side draws the plugin's text diagram.
        if id == konoma_mermaid::MERMAID_ID && !self.mermaid_image_mode() {
            return;
        }
        self.start_media_plugin_with(plugin, id, path);
    }

    /// [`App::start_media_plugin`] with the plugin already in hand (the tests hand in a stand-in).
    pub(super) fn start_media_plugin_with(
        &mut self,
        plugin: Arc<dyn Plugin>,
        id: &str,
        path: &Path,
    ) {
        // The show of the page before is kept (and its session reused) when it is this file's;
        // anything else is dropped and a fresh one begins.
        let kept = self
            .plugin
            .as_mut()
            .filter(|p| p.id == id && p.path == path)
            .map(|p| p.session.take());
        let session = match kept {
            Some(session) => session,
            None => {
                self.drop_plugin();
                self.plugin = Some(PluginShow {
                    id: id.to_string(),
                    path: path.to_path_buf(),
                    phase: PluginPhase::Loading,
                    session: None,
                    layout: konoma_plugin::view::Layout::single(),
                    view: None,
                    req: ViewReq::default(),
                    enabled: Default::default(),
                    flow: Default::default(),
                });
                None
            }
        };
        // The page asked for, counted from 0 in the request.
        let req = self
            .plugin_view_req()
            .page(self.tab.page.max(1).saturating_sub(1) as usize);
        let (job, wait) = match session {
            Some(session) => (PluginJob::View { session }, Wait::Nothing),
            None => (
                PluginJob::Open {
                    plugin,
                    input: Input::File {
                        path: path.to_path_buf(),
                        hint: None,
                    },
                },
                Wait::Layout(super::plugin_host::first_frame_budget()),
            ),
        };
        let work = self.plugin_work(id.to_string(), path.to_path_buf(), job, req);
        self.dispatch_plugin_work(work, wait);
    }
}

/// Test seam: the picture an SVG file gives when drawn at `max_px` through the `svg` plugin, run on
/// the spot (the same job the worker runs, unwrapped to its picture payload).
#[cfg(test)]
pub(crate) fn svg_file_payload(path: &Path, max_px: u32) -> Option<MediaPayload> {
    let mut cfg = Config::default();
    cfg.ui.svg_max_px = max_px;
    let dir = path.parent().unwrap_or(Path::new("/"));
    let app = App::new(dir.to_path_buf(), cfg).ok()?;
    let job = PluginJob::Open {
        plugin: Arc::new(konoma_media::SvgPlugin),
        input: Input::File {
            path: path.to_path_buf(),
            hint: None,
        },
    };
    app.plugin_work(
        konoma_media::SVG_ID.to_string(),
        path.to_path_buf(),
        job,
        ViewReq::default(),
    )
    .run_on_the_spot()
    .into_image_payload()
}

/// Test seam: what the host makes of an SVG a plugin returned for the file `shown`, with the
/// plugin's `base_dir` and its `trusted` claim; `trust_own` is whether the host believes the claim
/// (a standard plugin that draws its own SVG).
#[cfg(test)]
pub(crate) fn svg_view_payload(
    data: &[u8],
    base_dir: Option<PathBuf>,
    claims_trusted: bool,
    trust_own: bool,
    shown: &Path,
) -> Option<MediaPayload> {
    let view = ImageView {
        data: ImageData::Svg {
            data: Arc::from(data),
            base_dir,
            trusted: claims_trusted,
        },
        sharp_zoom: true,
        raster_max_px: None,
    };
    decode_image_view(
        view,
        shown,
        100,
        None,
        trust_own,
        &konoma_plugin::ReadScope::unrestricted(),
    )
}

#[cfg(test)]
#[path = "media_plugin_tests.rs"]
mod tests;
