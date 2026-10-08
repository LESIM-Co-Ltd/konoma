//! Word document preview (`PreviewKind::Document`): the converted Markdown and its pictures live
//! on `App`, are drawn by the ordinary Markdown pipeline, and are never written back to the file.
//!
//! The document is converted on the Office worker slot (`MediaJob::Document`); this module holds
//! what arrives (`LoadedDocument`), the App-level accessors the renderer / key handlers share, and
//! the `R` raw view (the converted Markdown written to a private temp file so the less-style reader
//! can window it).

use super::*;
use std::collections::HashMap;

/// One picture of the open document, held in memory only.
pub struct DocPicture {
    /// The file's bytes (png, jpeg, gif, bmp, webp, tiff or svg), shared with decode threads.
    pub(super) bytes: Arc<Vec<u8>>,
    /// Pixel size read from the header (raster) or the intrinsic size (SVG), without decoding.
    /// `None` = not an image konoma can size: it is drawn as its alt text.
    pub(super) dims: Option<(u32, u32)>,
    /// `dims` came from a raster header (not an SVG's intrinsic size): only those are refused
    /// from the header alone; an SVG is guarded by its drawing process instead.
    pub(super) raster: bool,
}

/// A converted Word document.
pub struct LoadedDocument {
    /// The Markdown the renderer draws (never read from disk).
    pub(super) markdown: String,
    /// The conversion stopped at one of its budgets (the end, some pictures, or part of a table is
    /// missing).
    pub(super) truncated: bool,
    /// Pictures by their `office-img://…` URL.
    pub(super) pictures: HashMap<String, DocPicture>,
    /// A presentation: its slides in order (empty for a Word document).
    pub(super) slides: Vec<crate::preview::office::docx::pptx::SlideInfo>,
    /// 0-based line of each slide's `## ` heading in `markdown`, in slide order (the reader keeps
    /// exactly one such line per slide). Found once here, on the worker, for the `R` raw view.
    pub(super) slide_lines: Vec<usize>,
    /// Byte offset in `markdown` of the last slide's heading line (0 without slides): where the
    /// raw view's window may be scrolled to, past its usual last page.
    pub(super) last_slide_byte: u64,
}

impl LoadedDocument {
    /// Builds the App-side form on the worker thread (sizes are read here, so the UI thread never
    /// parses an image header).
    pub(super) fn from_document(doc: crate::preview::office::docx::Document) -> Self {
        let pictures = doc
            .images
            .into_iter()
            .map(|im| {
                let (dims, raster) = picture_dims(&im.bytes);
                (
                    im.key,
                    DocPicture {
                        bytes: Arc::new(im.bytes),
                        dims,
                        raster,
                    },
                )
            })
            .collect();
        let slide_lines = if doc.slides.is_empty() {
            Vec::new()
        } else {
            doc.markdown
                .lines()
                .enumerate()
                .filter(|(_, l)| l.starts_with("## "))
                .map(|(i, _)| i)
                .collect()
        };
        let last_slide_byte = slide_lines.last().map_or(0, |last| {
            doc.markdown
                .split_inclusive('\n')
                .take(*last)
                .map(str::len)
                .sum::<usize>() as u64
        });
        LoadedDocument {
            markdown: doc.markdown,
            truncated: doc.truncated,
            pictures,
            slides: doc.slides,
            slide_lines,
            last_slide_byte,
        }
    }
}

/// Whether `path` is a presentation by its extension (PowerPoint or OpenDocument; the old binary
/// `.ppt` included, which is told apart as unreadable). Decides which reader the worker calls and
/// which words an error screen uses.
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

/// An old binary Office file (`.doc`, OLE/CFB) opened as a `.docx` is not a damaged zip, but when
/// its compound-file structure cannot be read the zip reader reports it as `Corrupt` (a valid one
/// is already `Unsupported`), which says "damaged". Looks at the first 8 bytes (the CFB signature)
/// and reports `Unsupported` ("not a Word format konoma reads, an old .doc for one") instead.
/// Every other error is returned as it came.
pub(super) fn legacy_binary_reason(
    path: &Path,
    err: crate::preview::office::OfficeError,
) -> crate::preview::office::OfficeError {
    use crate::preview::office::OfficeError;
    use std::io::Read as _;
    const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if !matches!(err, OfficeError::Corrupt(_)) {
        return err;
    }
    let mut head = [0u8; 8];
    let is_cfb = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && head == CFB_MAGIC;
    if is_cfb {
        OfficeError::Unsupported
    } else {
        err
    }
}

impl App {
    /// Replaces the open Word document. **The one place a document is dropped**: its pictures can
    /// total ~100 MiB, so the old one is freed off the UI thread (`discard_in_background`).
    pub(super) fn set_document(&mut self, doc: Option<Box<LoadedDocument>>) {
        if let Some(old) = std::mem::replace(&mut self.document, doc) {
            super::table_actions::discard_in_background(old);
        }
    }

    /// Whether the preview is a Word document (loaded or not).
    pub fn is_document(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Document(_)))
    }

    /// Whether the converted document is here and can be drawn.
    pub fn document_ready(&self) -> bool {
        self.is_document() && self.document.is_some()
    }

    /// A Word/ODF document whose text is not on screen: its conversion is still running (also
    /// when the tab came back in the raw `R` view) or it failed. The one predicate the footer, the
    /// `?` help and the text keys (`/`, `v`, `V`) share, so what is offered is what acts
    /// ([[hint-shown-iff-key-acts]]). A *re*load keeps the old text and is not "missing".
    pub fn document_text_missing(&self) -> bool {
        self.is_document() && !self.document_ready()
    }

    /// True while a Word document's worker has not delivered yet (the "loading" screen). A *re*load
    /// keeps the previous text on screen instead.
    pub fn is_document_loading(&self) -> bool {
        self.is_document()
            && self.document.is_none()
            && self.document_error.is_none()
            && self.media_loading
    }

    /// Why the document could not be shown (`None` while loading or on success).
    pub fn document_error(&self) -> Option<&crate::preview::office::OfficeError> {
        self.document_error.as_ref()
    }

    /// Whether the conversion stopped at a budget (the title says so).
    pub fn document_truncated(&self) -> bool {
        self.document.as_ref().is_some_and(|d| d.truncated)
    }

    /// The converted Markdown, when there is one for the document on screen.
    pub(super) fn document_markdown(&self) -> Option<&str> {
        if !self.is_document() {
            return None;
        }
        self.document.as_ref().map(|d| d.markdown.as_str())
    }

    /// Test-only: the converted Markdown.
    #[cfg(test)]
    pub fn document_markdown_for_test(&self) -> Option<&str> {
        self.document_markdown()
    }

    /// Test-only: forgets why the document failed (the state of a worker whose job produced
    /// neither a document nor a reason).
    #[cfg(test)]
    pub fn forget_document_error_for_test(&mut self) {
        self.document_error = None;
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

    /// Test-only: a weak handle on the bytes of picture `url` (alive while the document, or a
    /// decode of it, still holds them).
    #[cfg(test)]
    pub fn document_picture_weak_for_test(&self, url: &str) -> Option<std::sync::Weak<Vec<u8>>> {
        Some(Arc::downgrade(&self.document_picture_bytes(url)?))
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

    /// The bytes of one picture of the open document (shared, not copied).
    pub(super) fn document_picture_bytes(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        Some(self.document.as_ref()?.pictures.get(url)?.bytes.clone())
    }

    /// What arrived from the worker for a Word document. Dropped (off the UI thread) if the preview
    /// has since moved to something else.
    pub(super) fn land_document(&mut self, doc: Box<LoadedDocument>) {
        if !self.is_document() {
            super::table_actions::discard_in_background(doc);
            return;
        }
        // Pictures are filed under a hash of their bytes, so a reload cannot leave a *wrong* entry
        // behind, only unreferenced ones: reclaim those.
        self.md_image_cache.retain(|k, _| {
            let s = k.to_string_lossy();
            !crate::preview::markdown::is_office_image_url(&s) || doc.pictures.contains_key(&*s)
        });
        self.set_document(Some(doc));
        self.document_error = None;
        self.md_cache = None;
        // A search in force ran on the old text; its hits are re-scanned lazily by the next `n`.
        if self.tab.md_raw {
            // Raw view: the temp file holds the old text. Replace it and reopen the reader.
            self.clear_command_out();
            if self.write_document_raw() {
                self.setup_windowed();
            }
        }
    }

    // --- Slides of a presentation (`J`/`K`, the chip, the hints) -------------------------------

    /// Logical lines of the slides' headings in the decorated view, or `None` when they are not
    /// exactly the open presentation's slides (then nothing is offered).
    fn slide_head_lines(&self, c: &MdCache) -> Option<Vec<usize>> {
        let d = self.document.as_ref().filter(|_| self.is_document())?;
        if d.slides.is_empty() {
            return None;
        }
        let heads: Vec<usize> = c
            .anchors
            .iter()
            .filter(|(_, line)| {
                c.lines.get(*line).is_some_and(|l| {
                    crate::preview::markdown::heading_level_hint(
                        l,
                        crate::preview::markdown::row_after_heading(&c.lines, *line),
                    ) == 2
                })
            })
            .map(|(_, line)| *line)
            .collect();
        (heads.len() == d.slides.len()).then_some(heads)
    }

    /// The slides' headings as the view shows them, with the view's top: `(heads, top)`. Decorated
    /// view: display rows (`preview_scroll`); `R` raw view: lines of the Markdown
    /// (`preview_top_line`). `None` when this is not a presentation on screen, or its headings are
    /// not exactly its slides.
    fn slide_view(&self) -> Option<(Vec<usize>, usize)> {
        let d = self.document.as_ref().filter(|_| self.is_document())?;
        if d.slides.is_empty() {
            return None;
        }
        if self.tab.md_raw {
            if !self.is_windowed() || d.slide_lines.len() != d.slides.len() {
                return None;
            }
            return Some((d.slide_lines.clone(), self.tab.preview_top_line));
        }
        let c = self.md_cache.as_ref()?;
        let heads: Vec<usize> = self
            .slide_head_lines(c)?
            .into_iter()
            .map(|line| self.md_visual_span(line).0)
            .collect();
        Some((heads, self.tab.preview_scroll as usize))
    }

    /// The furthest the decorated view of a document may scroll: `total_rows - viewport` as for any
    /// text, but for a presentation at least the last slide's heading row, so every slide (the
    /// last ones too) can be put at the top and `J`/`K` stop on each. Below the last slide is then
    /// blank, as below the last page of a PDF. The one limit `G`, `j`, `Space`, the outline, the
    /// search and a resize all end up clamped to (the draw path applies it).
    pub(crate) fn slide_scroll_limit(&self, total_rows: usize, viewport: usize) -> usize {
        let base = total_rows.saturating_sub(viewport);
        let last_head = self
            .md_cache
            .as_ref()
            .filter(|_| !self.tab.md_raw)
            .and_then(|c| self.slide_head_lines(c))
            .and_then(|h| h.last().copied())
            .map(|line| self.md_visual_span(line).0);
        last_head.map_or(base, |h| base.max(h))
    }

    /// The raw view's counterpart of `slide_scroll_limit`: `(byte, line)` of the last slide's
    /// heading when the window may be scrolled to it (a presentation in the `R` view), to be taken
    /// when it lies past the last page. `None` for everything else, which keeps its last page.
    pub(crate) fn slide_raw_floor(&self) -> Option<(u64, usize)> {
        if !self.tab.md_raw || !self.is_document() {
            return None;
        }
        let d = self.document.as_ref()?;
        if d.slides.is_empty() || d.slide_lines.len() != d.slides.len() {
            return None;
        }
        Some((d.last_slide_byte, *d.slide_lines.last()?))
    }

    /// `(n, total)`: the slide at the top of the view (1-based; hidden slides count) of an open
    /// presentation. The chip in the status line.
    pub fn slide_position(&self) -> Option<(usize, usize)> {
        let (heads, top) = self.slide_view()?;
        let cur = slide_at(&heads, top)?;
        Some((cur + 1, heads.len()))
    }

    /// Whether `J`/`K` can move between slides now: a presentation of 2+ slides whose text is on
    /// screen. The one predicate the footer, the `?` help and `slide_turn` share
    /// ([[hint-shown-iff-key-acts]]).
    pub fn slide_can_turn(&self) -> bool {
        self.slide_view()
            .is_some_and(|(heads, ..)| heads.len() >= 2)
    }

    /// `J`/`K` on a presentation: scrolls the view to the next / previous slide's heading.
    pub(super) fn slide_turn(&mut self, dir: i32) {
        if !self.slide_can_turn() {
            return;
        }
        let Some((heads, top)) = self.slide_view() else {
            return;
        };
        let Some(i) = slide_turn_target(&heads, top, dir) else {
            return;
        };
        let at = heads[i];
        if self.tab.md_raw {
            if let Some(win) = self.preview_win.as_mut() {
                if let Ok((off, _)) = win.advance(0, at) {
                    self.tab.preview_byte_top = off;
                    self.tab.preview_top_line = at;
                    self.tab.preview_cursor_line = at;
                }
            }
        } else {
            self.tab.preview_scroll = at.min(u16::MAX as usize) as u16;
        }
    }

    /// Writes the converted Markdown to a private temp file and records it as the windowed reader's
    /// source (`tab.command_out`, see `App::windowed_src`). Returns whether there was text to write
    /// and the write worked. The file is deleted by `clear_command_out` (leaving the raw view, a
    /// new preview target, leaving Preview, closing the tab).
    pub(super) fn write_document_raw(&mut self) -> bool {
        let Some(md) = self.document_markdown() else {
            return false;
        };
        match crate::preview::command::write_private_temp(md.as_bytes()) {
            Ok(p) => {
                self.tab.command_out = Some(p);
                true
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
                false
            }
        }
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
