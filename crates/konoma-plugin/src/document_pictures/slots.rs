//! How much room each picture of a document takes, decided from what the cache holds: the
//! answers a host gives the document renderer ([`crate::document_render::PictureSlots`]) while
//! it lays the page out.
//!
//! A picture that is not there yet is `Loading`; one that cannot be shown is `Failed` with the
//! reason (said once: the entry keeps it) or `Unavailable`/`Text`/`Raw`, so the page degrades to
//! text instead of leaving a blank. A picture's reserved size comes from its first result's
//! `layout_px` and survives the eviction of its pixels, so it never changes under the reader.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::InlineImageCache;
use konoma_widgets::markdown::{
    is_remote_image_url, math_cells, math_url, md_image_cells, mermaid_cells, mermaid_fence_url,
    ImageSlot, MathSlot, MermaidSlot,
};

use super::files::{md_image_dims_why, resolve_md_image_path};
use crate::document_render::PictureSlots;

/// Max reserved rows for one inline image; a taller image is scaled down (keeping its aspect) so
/// it never dominates the viewport.
pub const MD_IMAGE_MAX_ROWS: u16 = 24;

/// What a host knows that decides the size of a document's pictures.
pub struct PictureLookup<'a> {
    pub cache: &'a InlineImageCache,
    /// URLs whose download failed (they are text, not `Loading`).
    pub remote_failed: &'a HashSet<String>,
    /// The terminal's cell size in pixels (width, height). `None` (no graphics backend) leaves
    /// every picture as text.
    pub font: Option<(u16, u16)>,
    /// The width, in cells, the page leaves a picture.
    pub avail: u16,
    /// The directory relative image paths are resolved against (the document's).
    pub base_dir: Option<PathBuf>,
    /// The most rows a mermaid fence may take.
    pub fence_target_rows: u16,
    /// Whether mermaid fences are drawn as pictures / math is.
    pub mermaid_on: bool,
    pub math_on: bool,
    /// The plugin that shows the document (named in a refusal).
    pub plugin_id: &'a str,
    /// Says why a file is refused by the read scope: the id of the plugin it was refused for, or
    /// `None` when it may be read. Recorded in `refusals` so the host can tell the user once.
    pub refused: &'a dyn Fn(&str, &Path) -> Option<String>,
    pub refusals: &'a RefCell<Vec<String>>,
    /// The text of a failure in the user's language (`plugin_id` is what a refusal names).
    pub message: &'a dyn Fn(ImageFailure, &str) -> Option<String>,
    /// A picture of a kind only the host knows (a Word document's embedded one): asked first,
    /// after the font check. `None` means "not mine".
    pub other: &'a dyn Fn(&str, Option<u16>) -> Option<ImageSlot>,
}

impl PictureLookup<'_> {
    fn failed(&self, f: ImageFailure, plugin_id: &str) -> ImageSlot {
        match (self.message)(f, plugin_id) {
            Some(why) => ImageSlot::Failed(why),
            None => ImageSlot::Unavailable,
        }
    }

    /// The slot of the image `url`. `max_cols` is the caller's own width budget when it has a
    /// narrower one than the page's (a table cell); `None` is the whole page.
    pub fn image(&self, url: &str, max_cols: Option<u16>) -> ImageSlot {
        let Some((fw, fh)) = self.font else {
            return ImageSlot::Unavailable;
        };
        if let Some(slot) = (self.other)(url, max_cols) {
            return slot;
        }
        let cells = |pw, ph| {
            let (cols, rows) = md_image_cells(
                pw,
                ph,
                fw,
                fh,
                max_cols.unwrap_or(self.avail),
                MD_IMAGE_MAX_ROWS,
            );
            ImageSlot::Inline { cols, rows }
        };
        let pid = self.plugin_id;
        if let Some(p) = resolve_md_image_path(url, self.base_dir.as_deref()) {
            // A document a plugin the user added returned: its pictures are read only inside
            // that plugin's read scope. Nothing of a refused file (not even its header) is read.
            if let Some(id) = (self.refused)(url, &p) {
                self.refusals.borrow_mut().push(id.clone());
                return self.failed(ImageFailure::OutsideReadDirs, &id);
            }
            // A picture that already failed to decode says why (the entry keeps the reason).
            if let Some(e) = self.cache.cache.get(&p).filter(|e| e.failed) {
                return match e.fail.and_then(|f| (self.message)(f, pid)) {
                    Some(why) => ImageSlot::Failed(why),
                    None => ImageSlot::Unavailable,
                };
            }
            // One whose header already says it is over the limits is refused here, before a
            // decode is queued for it (the decoder would refuse it from the same header).
            if let Some(d) = konoma_svg::image::dimensions(&p) {
                if !konoma_svg::image::dimensions_within_limits(d) {
                    return self.failed(ImageFailure::TooLarge, pid);
                }
            }
            match md_image_dims_why(&p) {
                Ok((pw, ph)) => cells(pw, ph),
                // An SVG refused from its size or its file (too large, a pipe): say why.
                Err(Some(why)) => self.failed(ImageFailure::Svg(why), pid),
                Err(None) => ImageSlot::Unavailable,
            }
        } else if is_remote_image_url(url) && !self.remote_failed.contains(url) {
            ImageSlot::Loading
        } else {
            ImageSlot::Unavailable
        }
    }

    /// The slot of a mermaid fence. The empty string is the probe ("are fences drawn as
    /// pictures?"), answered by the mode alone.
    pub fn mermaid(&self, code: &str) -> MermaidSlot {
        if !self.mermaid_on {
            return MermaidSlot::Text;
        }
        if code.is_empty() {
            return MermaidSlot::Loading;
        }
        let Some((fw, fh)) = self.font else {
            return MermaidSlot::Text;
        };
        let key = PathBuf::from(mermaid_fence_url(code));
        match self.cache.cache.get(&key) {
            Some(e) if e.failed => MermaidSlot::Text,
            // `layout_px` is set when the first result lands and survives eviction of the
            // pixels, so a picture whose pixels were dropped keeps its reserved size.
            Some(e) => match e
                .decoded
                .as_ref()
                .map(|i| image::GenericImageView::dimensions(i.as_ref()))
                .or(e.layout_px)
            {
                Some(raster_px) => {
                    // The layout is fixed at the **first** result's `layout_px`: the SVG's
                    // intrinsic size (px user units, the domain `mermaid_cells` sizes text
                    // against), not the raster's own pixel dimensions, so a sharp re-raster on
                    // zoom never changes the reserved cell count. The raster's dimensions
                    // stand in only if the intrinsic size could not be read.
                    let (pw, ph) = e.layout_px.unwrap_or(raster_px);
                    let (cols, rows) =
                        mermaid_cells(pw, ph, fw, fh, self.avail, self.fence_target_rows);
                    MermaidSlot::Image { cols, rows }
                }
                None => MermaidSlot::Loading,
            },
            None => MermaidSlot::Loading,
        }
    }

    /// The slot of a math expression.
    pub fn math(&self, latex: &str, display: bool) -> MathSlot {
        let Some((fw, fh)) = self.font else {
            return MathSlot::Raw;
        };
        let key = PathBuf::from(math_url(latex, display));
        match self.cache.cache.get(&key) {
            Some(e) if e.failed => MathSlot::Raw,
            // `layout_px` holds the SVG's **intrinsic size (em units)**, not raster px: rows come
            // from the em height and columns from the intrinsic aspect ratio, for a size balanced
            // against the text.
            Some(e) => match e.layout_px {
                Some((uw, uh)) => {
                    let (cols, rows) = math_cells(uw, uh, fw, fh, self.avail, display);
                    MathSlot::Image { cols, rows }
                }
                None => MathSlot::Loading,
            },
            None => MathSlot::Loading,
        }
    }

    /// Hand the document renderer these answers. `remote_on` is whether remote images are shown
    /// (and so collected for fetching).
    pub fn with_slots<R>(&self, remote_on: bool, f: impl FnOnce(&PictureSlots<'_>) -> R) -> R {
        let image = |u: &str, c: Option<u16>| self.image(u, c);
        let mermaid = |c: &str| self.mermaid(c);
        let math = |l: &str, d: bool| self.math(l, d);
        f(&PictureSlots {
            image: &image,
            mermaid: &mermaid,
            math: &math,
            mermaid_on: self.mermaid_on,
            math_on: self.math_on,
            remote_on,
        })
    }
}
