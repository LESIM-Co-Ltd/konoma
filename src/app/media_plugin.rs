//! The picture plugins (`image`, `svg`, `pdf`, `video`) on the host side.
//!
//! A picture plugin's `view` returns an [`ImageView`]; the host draws it with the image pipeline it
//! always had (decoding under the size limits, GIF frames, the kitty compressed transfer, zoom and
//! pan, sharp zoom of vector sources). The plugin's work and the decoding run **in one go on the
//! same media worker** ([`MediaPluginJob`]): the worker opens the plugin (or reuses the session of
//! the page before), asks it for the view, decodes what comes back, and delivers a
//! [`MediaPayload::Plugin`] — the decoded picture, the session and the layout it reported — through
//! the media channel under the media generation, so a result that is no longer wanted is dropped
//! on arrival exactly like every other media job. Without a channel (tests) the same job runs on
//! the spot.
//!
//! Pages: the layout a plugin reports (`Layout.pages`) is the host's page state
//! (`PerTab::page`, `PerTab::page_count`), and `J`/`K` ask the session for another page while the
//! old picture stays on screen.

use konoma_plugin::view::{ImageData, ImageView, Layout, PageLayout, View, ViewReq};
use konoma_plugin::{
    Cancel as PluginCancel, Failure, FailureKind, Input, OpenCx, Plugin, Session, Settings,
    TermCaps, ViewCx,
};

use super::*;

/// An open picture session kept between page turns (the PDF the tab shows).
pub(super) struct MediaSession {
    id: String,
    path: PathBuf,
    session: Box<dyn Session>,
}

/// Everything a media worker needs to run a picture plugin (owned: it moves to a thread).
pub(super) struct MediaPluginJob {
    plugin: Arc<dyn Plugin>,
    id: String,
    path: PathBuf,
    /// The open session of this file, when there is one (a page turn); otherwise `open` is called.
    session: Option<Box<dyn Session>>,
    /// The page asked for, counted from 1.
    page: u32,
    req: ViewReq,
    settings: Settings,
    lang: &'static str,
    pixels: bool,
    /// Whether the plugin may start an external program (`[external] pdf` / `video`).
    external: bool,
    /// The longest side to rasterize a vector source at when the plugin does not say.
    default_raster_px: u32,
}

/// What a picture plugin job delivers.
pub struct PluginLanded {
    id: String,
    path: PathBuf,
    /// The session, handed back for the next page turn (`None` when the plugin failed to open or
    /// panicked).
    session: Option<Box<dyn Session>>,
    /// What the session reported about its pages.
    layout: Option<Layout>,
    /// The decoded picture, or why the plugin could not make one.
    outcome: Result<Option<Box<MediaPayload>>, Failure>,
}

impl PluginLanded {
    /// The picture payload this landing stands for: the decoded picture, or the reason it could
    /// not be made (a failed read of an SVG file is an `ImageFailed` like a failed decode).
    #[cfg(test)]
    pub(super) fn into_image_payload(self) -> Option<MediaPayload> {
        outcome_payload(&self.id, self.outcome)
    }
}

/// The payload of a job's outcome (see [`PluginLanded::outcome`]).
fn outcome_payload(
    id: &str,
    outcome: Result<Option<Box<MediaPayload>>, Failure>,
) -> Option<MediaPayload> {
    match outcome {
        Ok(payload) => payload.map(|b| *b),
        Err(f) => image_failure_of(id, &f).map(MediaPayload::ImageFailed),
    }
}

/// The reason an image failure shows for a plugin's `Failure`. The pictures that have a fixed
/// fallback text of their own (a PDF, a video) show that text, so they have no reason here.
fn image_failure_of(id: &str, f: &Failure) -> Option<crate::preview::image::ImageFailure> {
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

impl MediaPluginJob {
    /// Open (or reuse the session), ask for the view, decode it. Never panics on its own; a plugin
    /// that panics takes the worker's `catch_silent` with it (no payload, as every media job).
    pub(super) fn run(self, cancel: Option<crate::preview::office::Cancel>) -> MediaPayload {
        let cancel_fn = {
            let cancel = cancel.clone();
            PluginCancel::from_fn(move || cancel.as_ref().is_some_and(|c| c.is_cancelled()))
        };
        let term = TermCaps {
            pixels: self.pixels,
        };
        let MediaPluginJob {
            plugin,
            id,
            path,
            session,
            page,
            req,
            settings,
            lang,
            external,
            default_raster_px,
            ..
        } = self;
        let landed = |session, layout, outcome| {
            MediaPayload::Plugin(Box::new(PluginLanded {
                id: id.clone(),
                path: path.clone(),
                session,
                layout,
                outcome,
            }))
        };
        let mut session = match session {
            Some(s) => s,
            None => {
                let cx = OpenCx::new(&settings, lang)
                    .with_cancel(cancel_fn.clone())
                    .with_term(term)
                    .with_external(external);
                let input = Input::File {
                    path: path.clone(),
                    hint: None,
                };
                match plugin.open(input, &cx) {
                    Ok(s) => s,
                    Err(f) => return landed(None, None, Err(f)),
                }
            }
        };
        let layout = session.layout();
        let cx = ViewCx::new(&settings, lang)
            .with_cancel(cancel_fn)
            .with_term(term)
            .with_external(external);
        let req = ViewReq {
            page: Some(page.saturating_sub(1) as usize),
            ..req
        };
        let outcome = match session.view(&req, &cx) {
            Ok(View::Image(iv)) => {
                Ok(decode_image_view(iv, &path, default_raster_px, cancel.as_ref()).map(Box::new))
            }
            Ok(_) => Err(Failure::unsupported(konoma_plugin::t!(
                en: "the plugin did not return a picture",
                ja: "プラグインが画像を返しませんでした",
            ))),
            Err(f) => Err(f),
        };
        landed(Some(session), Some(layout), outcome)
    }
}

/// Decode what a picture plugin returned into the host's media payload. `None` is a failure with
/// no reason to show (the render side falls back), as for any media job.
fn decode_image_view(
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
            let img = image::RgbaImage::from_raw(width, height, pixels.to_vec())?;
            Some(MediaPayload::Static(image::DynamicImage::ImageRgba8(img)))
        }
        // Not made by the picture plugins; a plugin that returns them gets the honest reason.
        ImageData::Encoded(_) | ImageData::Frames(_) | ImageData::Lazy(_) => {
            Some(MediaPayload::ImageFailed(ImageFailure::UnsupportedFormat))
        }
    }
}

impl App {
    /// Start showing the file `path` with the picture plugin `id` for the current preview: on the
    /// media worker (or on the spot without one), asking for the page the tab is on. The session of
    /// the page before is reused when it belongs to this file.
    pub(super) fn start_media_plugin(&mut self, id: &str, path: &Path) {
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        if !registry.is_enabled(id) {
            return;
        }
        let Some(plugin) = registry.plugin(id) else {
            return;
        };
        // A still image needs the decode backend too (its encode is made asynchronously at render
        // time); every other picture only needs a graphics backend. An animation is a "GIF" to
        // the host when its first bytes say so.
        let still = id == konoma_media::IMAGE_ID && !Self::looks_like_gif(path);
        if self.picker.is_none() || (still && self.img_tx.is_none()) {
            return; // the render side falls back to text or a message
        }
        let session = match self.media_session.take() {
            Some(s) if s.id == id && s.path == path => Some(s.session),
            _ => None,
        };
        let external = match id {
            konoma_media::PDF_ID => self.cfg.external.pdf,
            konoma_media::VIDEO_ID => self.cfg.external.video,
            _ => false,
        };
        let job = MediaPluginJob {
            plugin,
            id: id.to_string(),
            path: path.to_path_buf(),
            session,
            page: self.tab.page.max(1),
            req: self.plugin_view_req(),
            settings: crate::plugins::effective_settings(&self.cfg, id),
            lang: self.plugin_lang(),
            pixels: self.terminal_draws_pixels(),
            external,
            default_raster_px: self.cfg.ui.svg_max_px,
        };
        self.spawn_or_sync_media(MediaJob::Plugin(Box::new(job)));
    }

    /// A picture plugin's result landed (current generation): keep its session, take the page
    /// count it reported, then apply the picture.
    pub(super) fn land_media_plugin(&mut self, landed: Box<PluginLanded>) {
        let landed = *landed;
        // The preview moved on to something else that started no media job of its own.
        if !self
            .tab
            .preview_kind
            .as_ref()
            .is_some_and(|k| k.is_plugin(&landed.id))
        {
            return;
        }
        self.media_session = landed.session.map(|session| MediaSession {
            id: landed.id.clone(),
            path: landed.path.clone(),
            session,
        });
        if let Some(Layout {
            pages: Some(PageLayout::Separate { count, .. }),
            ..
        }) = &landed.layout
        {
            self.tab.page_count = count.and_then(|n| u32::try_from(n).ok());
            if let Some(n) = self.tab.page_count {
                self.tab.page = self.tab.page.clamp(1, n.max(1));
            }
        }
        if let Some(payload) = outcome_payload(&landed.id, landed.outcome) {
            self.apply_payload(payload);
        }
    }
}

/// Test seam: the picture an SVG file gives when drawn at `max_px` through the `svg` plugin, run on
/// the spot (the same job the worker runs, unwrapped to its picture payload).
#[cfg(test)]
pub(crate) fn svg_file_payload(path: &Path, max_px: u32) -> Option<MediaPayload> {
    let job = MediaPluginJob {
        plugin: Arc::new(konoma_media::SvgPlugin),
        id: konoma_media::SVG_ID.to_string(),
        path: path.to_path_buf(),
        session: None,
        page: 1,
        req: ViewReq::default(),
        settings: Settings::new().with("max_px", konoma_plugin::Value::Int(i64::from(max_px))),
        lang: "en",
        pixels: false,
        external: false,
        default_raster_px: max_px,
    };
    match job.run(None) {
        MediaPayload::Plugin(landed) => landed.into_image_payload(),
        other => Some(other),
    }
}

#[cfg(test)]
#[path = "media_plugin_tests.rs"]
mod tests;
