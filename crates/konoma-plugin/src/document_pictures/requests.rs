//! The requests a document's pictures make, and what becomes of their results.
//!
//! [`DocPictures`] is a view over the state a host keeps for the document it shows: the picture
//! cache, the remote downloads in flight and the ones that failed. The host builds one per call
//! from its own fields (the state stays where the host's other code reads it) and hands in what
//! only it knows: the senders the results travel back on, the SVG drawing, the read scope, the
//! setting that allows remote images. What is decided here is the same in every host:
//!
//! - a mermaid fence or a math expression is keyed by its source ([`konoma_widgets::markdown::
//!   mermaid_fence_url`], [`konoma_widgets::markdown::math_url`]), so an edited one renders fresh
//!   and an unchanged one reuses its raster;
//! - at most [`MAX_SYNTHETIC_RENDERS_IN_FLIGHT`] of them render at once;
//! - a remote image is fetched unless the host's setting turns remote images off, once per URL;
//! - a result answers only the request it was made for ([`DecodedImage::request`]), and a decode
//!   whose preview has moved on is stopped and reported as cancelled;
//! - a picture that left the document is swept from the cache.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::{DecodedImage, Filed, InlineImageCache, MdImgEntry, SvgTrust};
use konoma_widgets::markdown::{is_math_url, is_mermaid_fence_url, math_url, mermaid_fence_url};

use super::files::{looks_like_gif, md_decode_image_within, resolve_md_image_path};
use super::remote::{md_remote_cache_path, Fetcher, RemoteFetch};
use crate::canvas_pictures::{EmbedPicture, EmbedRender, SvgDraw};

/// Cap on how many mermaid-fence/math renders may be in flight (spawned, result not yet applied)
/// at once. Unlike a real inline image, which is only requested for a placement the renderer
/// actually draws, a fence/math render is requested for the *whole* document (its rendered size
/// has to be known just to lay the document out, even for content that is off-screen). Without a
/// cap, a document with hundreds of distinct expressions would call `std::thread::spawn` hundreds
/// of times in a single pass on the UI thread (measured: 600 expressions took 911ms, about 15
/// times the 60ms draw budget), and past some count `std::thread::spawn` can itself fail.
pub const MAX_SYNTHETIC_RENDERS_IN_FLIGHT: usize = 16;

/// What asking for a synthetic picture (a mermaid fence or a math expression) did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// Nothing started: the picture is cached already, or the cap is reached (the next pass,
    /// after a result lands and the host re-scans the document, asks again).
    Nothing,
    /// A worker thread renders it; the result comes back on the results channel.
    Started,
    /// No results channel: it was rendered here and filed; the host must rebuild the layout it
    /// just made, which still shows the loading line.
    Done,
}

/// What a result did, for the host to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// Whether anything on screen may have changed.
    pub redraw: bool,
    /// Whether the document's layout must be built again: a fence or formula decided its size for
    /// the first time, or a picture that cannot be shown changes what the page reserves for it
    /// (one line of text instead of rows of cells). Never for a sharpening re-raster.
    pub relayout: bool,
    /// Whether new pixels were filed (the host may now be over its memory budget).
    pub pixels: bool,
}

impl Applied {
    const NONE: Applied = Applied {
        redraw: false,
        relayout: false,
        pixels: false,
    };
}

/// What one decode needs of the host (`DocPictures::spawn_decode`).
pub struct DecodeJob {
    /// The longest side an SVG is drawn at.
    pub svg_max_px: u32,
    /// The folders an SVG may read (`None`: no limit).
    pub roots: Option<Vec<PathBuf>>,
    /// The file to read: the real path the read scope judged. `None` is a picture outside the
    /// scope, answered with [`ImageFailure::OutsideReadDirs`] without opening anything.
    pub read_path: Option<PathBuf>,
    /// Draws the SVGs.
    pub svg: Arc<dyn SvgDraw + Send + Sync>,
    /// The preview's generation when the job started, and the host's latest: the decode is
    /// stopped once they differ.
    pub generation: (u64, Arc<AtomicU64>),
}

/// How many mermaid-fence/math renders have a placeholder in `cache` but no result yet. Both
/// kinds share the same one-thread-per-request shape and the same cap.
pub fn synthetic_in_flight(cache: &InlineImageCache) -> usize {
    cache
        .cache
        .iter()
        .filter(|(k, e)| {
            if e.decoded.is_some() || e.failed || e.evicted {
                return false; // done (either way): no longer occupying a slot
            }
            let s = k.to_string_lossy();
            is_mermaid_fence_url(&s) || is_math_url(&s)
        })
        .count()
}

/// The document's picture state, borrowed from the host for one call.
pub struct DocPictures<'a> {
    pub cache: &'a mut InlineImageCache,
    pub remote_inflight: &'a mut HashSet<String>,
    pub remote_failed: &'a mut HashSet<String>,
    /// Where decode and render results go. `None` renders synchronously (tests, hosts without a
    /// loop to deliver to).
    pub results: Option<Sender<DecodedImage>>,
    /// Where download results go. `None` starts no download.
    pub fetches: Option<Sender<RemoteFetch>>,
    /// Whether remote (`http(s)://`) images are fetched at all (`ui.remote_images`, on by default).
    pub remote_images: bool,
    /// The download itself ([`super::fetch_remote_image`] in a host).
    pub fetcher: Fetcher,
}

impl DocPictures<'_> {
    /// How many mermaid-fence/math renders have a placeholder in the cache but no result yet.
    pub fn synthetic_in_flight(&self) -> usize {
        synthetic_in_flight(self.cache)
    }

    /// Whether the picture filed under `key` (a [`mermaid_fence_url`] or [`math_url`]) is in the
    /// cache already (rendered or in flight): a request for it does nothing.
    pub fn is_cached(&self, key: &Path) -> bool {
        self.cache.cache.contains_key(key)
    }

    /// Ensure a mermaid render is in flight (or cached) for one fence. `embed` is the plugin that
    /// draws it (`None`: switched off, the same as a render that failed). Building it is costly
    /// (registry and settings), so a host asks [`Self::is_cached`] before it builds one.
    pub fn request_fence(
        &mut self,
        code: String,
        embed: Option<EmbedRender>,
        kind: &'static str,
        max_px: u32,
    ) -> Requested {
        let key = PathBuf::from(mermaid_fence_url(&code));
        self.request_synthetic(
            key,
            embed,
            kind,
            &code,
            false,
            max_px,
            "mermaid",
            "mermaid.svg",
        )
    }

    /// Ensure a render is in flight (or cached) for one math expression, keyed by LaTeX and
    /// display mode. The SVG is kept so a result can record its intrinsic size for layout.
    pub fn request_math(
        &mut self,
        latex: String,
        display: bool,
        embed: Option<EmbedRender>,
        kind: &'static str,
        max_px: u32,
    ) -> Requested {
        let key = PathBuf::from(math_url(&latex, display));
        self.request_synthetic(
            key, embed, kind, &latex, display, max_px, "math", "math.svg",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn request_synthetic(
        &mut self,
        key: PathBuf,
        embed: Option<EmbedRender>,
        kind: &'static str,
        source: &str,
        display: bool,
        max_px: u32,
        label: &'static str,
        base: &'static str,
    ) -> Requested {
        if self.cache.cache.contains_key(&key) {
            return Requested::Nothing;
        }
        // The plugin that takes the kind draws the SVG; none is the same "cannot be drawn" as a
        // failed render.
        let svg_trust = embed
            .as_ref()
            .map_or(SvgTrust::Embedded, |e| e.svg_trust(base));
        let source = source.to_string();
        let render = move || -> EmbedPicture {
            match &embed {
                Some(e) => e.picture(kind, &source, display, max_px, label, base),
                None => (Err(format!("{label} render failed")), None),
            }
        };
        self.request_rendered(key, svg_trust, label, render)
    }

    /// Ensure `render` runs (on a worker, or here without a results channel) for the synthetic
    /// picture filed under `key`, unless it is cached already or the cap is reached. `label`
    /// names the picture kind in a panic's message. The core both [`Self::request_fence`] and
    /// [`Self::request_math`] go through.
    pub fn request_rendered(
        &mut self,
        key: PathBuf,
        svg_trust: SvgTrust,
        label: &'static str,
        render: impl FnOnce() -> EmbedPicture + Send + 'static,
    ) -> Requested {
        if self.cache.cache.contains_key(&key) {
            return Requested::Nothing;
        }
        if let Some(tx) = self.results.clone() {
            // At the cap, skip this pass *without* inserting a placeholder. Nothing is lost: a
            // result that lands asks the host to rebuild, and that rebuild re-scans the document
            // and asks again for whatever is still absent, now that a slot has freed up. The
            // wave converges without a dedicated worker thread.
            if self.synthetic_in_flight() >= MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
                return Requested::Nothing;
            }
            self.insert_placeholder(&key, svg_trust);
            let request = self.cache.next_request(&key);
            std::thread::spawn(move || {
                // Even if the render panics, always return a result: otherwise the entry stays
                // at decoded=None && !failed and the host's busy flag latches for good.
                let (image, svg) = konoma_base::panic::catch_silent(render)
                    .unwrap_or_else(|| (Err(format!("{label} render panicked")), None));
                let _ = tx.send(DecodedImage {
                    path: key,
                    image,
                    svg,
                    reraster: false,
                    frames: None,
                    request,
                });
            });
            Requested::Started
        } else {
            self.insert_placeholder(&key, svg_trust);
            let request = self.cache.next_request(&key);
            let (image, svg) = render();
            self.apply_result(DecodedImage {
                path: key,
                image,
                svg,
                reraster: false,
                frames: None,
                request,
            });
            Requested::Done
        }
    }

    fn insert_placeholder(&mut self, key: &Path, svg_trust: SvgTrust) {
        self.cache.cache.insert(
            key.to_path_buf(),
            MdImgEntry {
                svg_trust,
                ..MdImgEntry::default()
            },
        );
    }

    /// File a finished decode or render. For a mermaid fence or a formula the layout size is the
    /// SVG's **intrinsic size** (px user units), not the raster's pixel dimensions: the raster is
    /// scaled by an unrelated setting, and sizing off it would drag that setting into the
    /// text-size arithmetic.
    pub fn apply_result(&mut self, res: DecodedImage) -> Applied {
        let url = res.path.to_string_lossy().to_string();
        let is_math = is_math_url(&url);
        let is_mermaid = is_mermaid_fence_url(&url);
        let reraster = res.reraster;
        let intrinsic = if (is_math || is_mermaid)
            && res.image.is_ok()
            && self
                .cache
                .cache
                .get(&res.path)
                .is_some_and(|e| e.layout_px.is_none() && e.request == res.request)
        {
            res.svg
                .as_deref()
                .and_then(|d| konoma_svg::svg::intrinsic_size_bytes(d))
        } else {
            None
        };
        let filed = self.cache.file_decoded(res, intrinsic);
        // The entry is always pre-placed when the request starts. Missing means a stale result
        // (the document changed, the entry was swept or dropped): it is dropped instead of
        // reviving it, which would also rebuild the page of an unrelated document. A cancelled
        // decode forgot the entry.
        if matches!(filed, Filed::Dropped | Filed::Forgotten) {
            return Applied::NONE;
        }
        Applied {
            redraw: true,
            relayout: !reraster && (is_mermaid || is_math || filed == Filed::Failed),
            pixels: filed == Filed::Pixels,
        }
    }

    /// Ensure a download is in flight for the remote image `url` (deduplicated). Skips a URL
    /// that is cached already, downloading, or known to have failed. Returns true when the host
    /// must rebuild the layout it just made, which still shows a loading placeholder: remote
    /// images are off, so the URL was marked failed here, and nothing else would ever invalidate
    /// the layout (unlike a download that completes). Only the first time (`insert` reports
    /// whether the URL was new).
    pub fn request_remote(&mut self, url: &str) -> bool {
        if !konoma_widgets::markdown::is_remote_image_url(url) {
            return false;
        }
        if !self.remote_images {
            // Never call out. Mark it failed right away (instead of leaving it unrecorded) so the
            // renderer degrades to the text placeholder instead of waiting forever.
            return self.remote_failed.insert(url.to_string());
        }
        // Already downloaded (the cache file exists), already failed, or already downloading.
        if resolve_md_image_path(url, None).is_some()
            || self.remote_failed.contains(url)
            || self.remote_inflight.contains(url)
        {
            return false;
        }
        let (Some(tx), Some(dest)) = (self.fetches.clone(), md_remote_cache_path(url)) else {
            return false;
        };
        self.remote_inflight.insert(url.to_string());
        let u = url.to_string();
        let fetch = self.fetcher;
        std::thread::spawn(move || {
            // Always report, a panic included (as a failure): leaving the URL in the in-flight
            // set would latch the host's busy flag.
            let ok = konoma_base::panic::catch_silent(|| fetch(&u, &dest)).unwrap_or(false);
            let _ = tx.send(RemoteFetch { url: u, ok });
        });
        false
    }

    /// Take a finished download. On failure the URL is remembered, so it is not retried and shows
    /// a text placeholder. The host rebuilds its layout either way (a cached image is laid out
    /// inline).
    pub fn apply_fetch(&mut self, res: RemoteFetch) {
        self.remote_inflight.remove(&res.url);
        if !res.ok {
            self.remote_failed.insert(res.url);
        }
    }

    /// The first decode of a picture that has no cache entry: file the entry, stamp it as wanted
    /// by the pass being drawn (eviction order and decode priority) and start the decode. A
    /// thread that could not start ends the wait (`failed`) instead of latching "loading".
    pub fn begin_decode(&mut self, path: PathBuf, job: DecodeJob) {
        let frame = self.cache.frame;
        let entry = self.cache.cache.entry(path.clone()).or_default();
        entry.last_used = frame;
        entry.wish.set_priority(frame);
        if !self.spawn_decode(path.clone(), job) {
            if let Some(e) = self.cache.cache.get_mut(&path) {
                e.failed = true;
            }
        }
    }

    /// Start the decode of the picture file `path` (its cache entry exists already). The format
    /// is sniffed from the content (a cached download has no extension), an SVG is rasterized,
    /// and an animated GIF keeps all its frames. With no results channel nothing is started.
    /// Returns false when the thread could not be started (the caller must not leave the entry
    /// waiting).
    ///
    /// The decode queues in the shared decode gate under the entry's own ticket: what is on screen
    /// now goes first, and a decode whose entry was dropped meanwhile never runs.
    pub fn spawn_decode(&mut self, path: PathBuf, job: DecodeJob) -> bool {
        let Some(tx) = self.results.clone() else {
            return true;
        };
        let ticket = self
            .cache
            .cache
            .get(&path)
            .map(|e| e.wish.ticket())
            .unwrap_or_default();
        let request = self.cache.next_request(&path);
        let DecodeJob {
            svg_max_px,
            roots,
            read_path,
            svg,
            generation: (gen, latest),
        } = job;
        let stale = ticket.clone();
        std::thread::Builder::new()
            .name("konoma-md-decode".into())
            .spawn(move || {
                // An SVG is drawn by a process that is stopped as soon as the preview has moved on.
                let moved_on = || latest.load(Ordering::Relaxed) != gen || stale.is_stale();
                let p = path;
                let Some(rp) = read_path else {
                    // Outside the read scope: nothing is opened, the refusal is the answer.
                    let _ = tx.send(DecodedImage {
                        path: p,
                        image: Err(ImageFailure::OutsideReadDirs.code().to_string()),
                        svg: None,
                        reraster: false,
                        frames: None,
                        request,
                    });
                    return;
                };
                // Animated GIF: decode all frames so the picture cycles the way the full-screen
                // one does, with a smaller budget than the full-screen path (several GIFs at once
                // must fit). Anything that yields fewer than 2 frames falls through unchanged to
                // the normal still decode. A panic (pathological image/SVG) is caught too, and a
                // result always returned: not returning would latch busy.
                let (still, frames) = konoma_base::panic::catch_silent(|| {
                    konoma_svg::image::with_decode_ticket(ticket, || {
                        if looks_like_gif(&rp) {
                            if let Some(frames) = konoma_svg::image::decode_gif_inline(&rp) {
                                let first = frames[0].0.clone();
                                return (Ok(first), Some(frames));
                            }
                        }
                        (
                            md_decode_image_within(
                                &rp,
                                svg_max_px,
                                roots.as_deref(),
                                svg.as_ref(),
                                &moved_on,
                            ),
                            None,
                        )
                    })
                })
                .unwrap_or((Err(ImageFailure::Corrupt), None));
                // The preview moved on while the picture was being drawn: report it as cancelled.
                // The entry may be gone (the result is then ignored) or still in the cache (a
                // ticket also goes stale when another file starts a media load, which does not
                // clear the cache): only the receiving side knows, and it forgets the entry so
                // the picture is asked for again. Staying silent would leave it "loading" for good.
                let still = match still {
                    Err(_) if moved_on() => Err(ImageFailure::Cancelled),
                    other => other,
                };
                let image = still.map_err(|f| f.code().to_string());
                let _ = tx.send(DecodedImage {
                    path: p,
                    image,
                    svg: None,
                    reraster: false,
                    frames,
                    request,
                });
            })
            .is_ok()
    }

    /// Sweep the fence and math pictures that **left the document**. The key is a content hash,
    /// so an external edit generates a new key every time the content changes, and the old key's
    /// raster, protocol and SVG would otherwise grow until the file is switched. `live` holds the
    /// keys the document still has; every other picture is left alone.
    pub fn sweep(&mut self, live: &HashSet<PathBuf>) {
        self.cache.cache.retain(|k, _| {
            let s = k.to_string_lossy();
            !(is_mermaid_fence_url(&s) || is_math_url(&s)) || live.contains(k)
        });
    }
}

/// The cache keys of the fences and expressions a document has: what [`DocPictures::sweep`]
/// keeps.
pub fn live_synthetic_keys<'a>(
    fences: impl IntoIterator<Item = &'a String>,
    math: impl IntoIterator<Item = &'a (String, bool)>,
) -> HashSet<PathBuf> {
    let mut live: HashSet<PathBuf> = fences
        .into_iter()
        .map(|c| PathBuf::from(mermaid_fence_url(c)))
        .collect();
    live.extend(
        math.into_iter()
            .map(|(l, d)| PathBuf::from(math_url(l, *d))),
    );
    live
}
