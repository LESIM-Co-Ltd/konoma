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

use konoma_plugin::view::{ImageData, ImageView, ViewReq};
use konoma_plugin::{Failure, FailureKind, Input, Plugin};

use super::plugin_host::{PluginJob, PluginPhase, PluginShow, Wait, FIRST_FRAME_BUDGET};
use super::*;

/// The reason an image failure shows for a plugin's `Failure`. The pictures that have a fixed
/// fallback text of their own (a PDF, a video) show that text, so they have no reason here.
pub(super) fn image_failure_of(
    id: &str,
    f: &Failure,
) -> Option<crate::preview::image::ImageFailure> {
    use crate::preview::image::ImageFailure;
    use crate::preview::svg_guard::SvgFail;
    match (id, f.kind) {
        (_, FailureKind::Cancelled) => Some(ImageFailure::Cancelled),
        (konoma_media::SVG_ID, FailureKind::TooLarge) => Some(ImageFailure::Svg(SvgFail::TooLarge)),
        (konoma_media::SVG_ID, _) => Some(ImageFailure::Svg(SvgFail::Invalid)),
        (konoma_media::IMAGE_ID, FailureKind::TooLarge | FailureKind::TooHeavy) => {
            Some(ImageFailure::TooLarge)
        }
        (konoma_media::IMAGE_ID, FailureKind::Unsupported) => Some(ImageFailure::UnsupportedFormat),
        (konoma_media::IMAGE_ID, _) => Some(ImageFailure::Corrupt),
        _ => None,
    }
}

/// Decode what a picture plugin returned into the host's media payload. `None` is a failure with
/// no reason to show (the render side falls back), as for any media job.
pub(super) fn decode_image_view(
    iv: ImageView,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
) -> Option<MediaPayload> {
    use crate::preview::image::ImageFailure;
    let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    let max_px = iv.raster_max_px.unwrap_or(default_raster_px);
    match iv.data {
        // A file: the host tells an animation from a still by the first bytes, and decodes it
        // under its size limits and the shared decode gate.
        ImageData::File(p) => Some(if App::looks_like_gif(&p) {
            match crate::preview::image::decode_gif(&p) {
                Some(frames) => MediaPayload::Gif(frames),
                // A single-frame / non-animated GIF is shown as a still image.
                None => still_payload(&p),
            }
        } else {
            still_payload(&p)
        }),
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => {
            // Relative references inside the SVG resolve from the directory the plugin names.
            let base = match (&base_dir, path.file_name()) {
                (Some(dir), Some(name)) => dir.join(name),
                _ => path.to_path_buf(),
            };
            let img = if trusted {
                crate::preview::svg::rasterize_trusted(&data, &base, max_px)?
            } else {
                // A file the user opened: drawn by a supervised child process.
                match crate::preview::svg::rasterize_untrusted(&data, &base, max_px, &cancelled) {
                    Ok(img) => img,
                    Err(why) => return Some(MediaPayload::ImageFailed(why.into())),
                }
            };
            Some(if iv.sharp_zoom {
                MediaPayload::Vector {
                    img,
                    svg: Arc::new(data.to_vec()),
                }
            } else {
                MediaPayload::Static(img)
            })
        }
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            // The plugin handed over the only reference: the buffer is taken, not copied.
            let raw = Arc::try_unwrap(pixels).unwrap_or_else(|shared| (*shared).clone());
            let img = image::RgbaImage::from_raw(width, height, raw)?;
            Some(MediaPayload::Static(image::DynamicImage::ImageRgba8(img)))
        }
        // Not made by the picture plugins; a plugin that returns them gets the honest reason.
        ImageData::Encoded(_) | ImageData::Frames(_) | ImageData::Lazy(_) => {
            Some(MediaPayload::ImageFailed(ImageFailure::UnsupportedFormat))
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only: a plugin that stands in for whatever the registry would give, on this thread.
    pub(super) static STAND_IN: std::cell::RefCell<Option<Arc<dyn Plugin>>> =
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
        let still = id == konoma_media::IMAGE_ID && !Self::looks_like_gif(path);
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
                });
                None
            }
        };
        // The page asked for, counted from 0 in the request.
        let req = ViewReq {
            page: Some(self.tab.page.max(1).saturating_sub(1) as usize),
            ..self.plugin_view_req()
        };
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
                Wait::Layout(FIRST_FRAME_BUDGET),
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

#[cfg(test)]
#[path = "media_plugin_tests.rs"]
mod tests;
