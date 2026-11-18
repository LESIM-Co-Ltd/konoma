//! Filing and rebuilding media pixels through the existing document picture requests.
use super::{Applied, DocPictures};
use crate::canvas_pictures::SvgDraw;
use crate::host::project::media_diff::MediaDiffPictureDecoded;
use konoma_widgets::inline_images::{DecodedImage, SvgTrust};
use std::sync::{atomic::AtomicU64, Arc};

const MAX_MEDIA_SVG_REBUILDS: usize = 4;

impl DocPictures<'_> {
    /// Pre-place an accepted worker picture before the ordinary stale-result guard runs.
    pub fn place_media_diff_picture(&mut self, p: MediaDiffPictureDecoded) -> Applied {
        let entry = self.cache.cache.entry(p.cache_key.clone()).or_default();
        entry.svg_trust = match p.svg_base {
            Some(base) => SvgTrust::File(base),
            None => SvgTrust::Embedded,
        };
        let natural_px = p.natural_px;
        let key = p.cache_key.clone();
        let request = self.cache.next_request(&p.cache_key);
        let applied = self.apply_result(DecodedImage {
            path: p.cache_key,
            image: Ok(p.image),
            svg: p.svg,
            reraster: false,
            frames: p.frames,
            request,
        });
        // Size the accepted decode before exposing it. Media frames use the outcome's intrinsic
        // size, just like konoma; document lookups retain it when rebuilding an evicted SVG.
        if applied.pixels {
            if let Some(entry) = self.cache.cache.get_mut(&key) {
                entry.layout_px = Some(natural_px);
            }
        }
        applied
    }

    /// Rebuild retained media SVG pixels on a worker through the host's supervised drawer.
    pub fn rebuild_media_svg(
        &mut self,
        path: &std::path::Path,
        max_px: u32,
        drawer: Arc<dyn SvgDraw + Send + Sync>,
        generation: (u64, Arc<AtomicU64>),
    ) {
        if !konoma_widgets::media_diff::is_media_diff_url(&path.to_string_lossy()) {
            return;
        }
        let Some(tx) = self.results.clone() else {
            return;
        };
        if self.cache.cache.values().filter(|e| e.rebuilding).count() >= MAX_MEDIA_SVG_REBUILDS {
            return;
        }
        let Some(entry) = self.cache.cache.get_mut(path) else {
            return;
        };
        if !entry.evicted || entry.decoded.is_some() || entry.failed || entry.rebuilding {
            return;
        }
        let Some(svg) = entry.svg.clone() else { return };
        let trust = entry.svg_trust.clone();
        let ticket = entry.wish.ticket();
        entry.rebuilding = true;
        entry.reraster_inflight = true;
        let request = self.cache.next_request(path);
        let path = path.to_path_buf();
        let key = path.clone();
        let (gen, generation) = generation;
        let started = std::thread::Builder::new()
            .name("konoma-md-rebuild".into())
            .spawn(move || {
                let image = konoma_base::panic::catch_silent(|| {
                    crate::canvas_pictures::redraw_svg(
                        &svg,
                        &trust,
                        max_px,
                        drawer.as_ref(),
                        &|| {
                            generation.load(std::sync::atomic::Ordering::Relaxed) != gen
                                || ticket.is_stale()
                        },
                    )
                    .map_err(|_| "rasterize failed".to_string())
                })
                .unwrap_or_else(|| Err("re-raster panicked".to_string()));
                let _ = tx.send(DecodedImage {
                    path,
                    image,
                    svg: None,
                    reraster: true,
                    frames: None,
                    request,
                });
            })
            .is_ok();
        if !started {
            if let Some(entry) = self.cache.cache.get_mut(&key) {
                entry.rebuilding = false;
                entry.reraster_inflight = false;
                entry.failed = true;
            }
        }
    }
}
