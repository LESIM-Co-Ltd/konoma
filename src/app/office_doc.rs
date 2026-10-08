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

/// The state of a presentation without the worker: a converted deck put straight into an `App`, the
/// raw (`R`) view's reader opened on its Markdown. Pins what `slide_view` / `slide_raw_floor` /
/// `win_max_top` and the key handlers do with exact numbers, including the states the keys cannot
/// reach one by one (a window that is not open, a Markdown whose headings are not its slides).
#[cfg(test)]
mod slide_state_tests {
    use super::*;
    use crate::preview::office::docx::pptx::SlideInfo;
    use crate::preview::office::docx::Document;

    /// Holds the `App` and the temp dir of its raw view; `Drop` deletes the private temp file.
    struct Rig {
        app: App,
        _dir: crate::test_support::TmpDir,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            self.app.clear_command_out();
        }
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

    /// Byte offset of the start of line `n` (0-based).
    fn byte_of_line(md: &str, n: usize) -> u64 {
        md.split_inclusive('\n')
            .take(n)
            .map(str::len)
            .sum::<usize>() as u64
    }

    /// An `App` that shows `doc` (as a `.pptx` or, with no slides, as a Word file) in the raw view,
    /// a `vh`-row viewport.
    fn raw_rig(doc: Document, vh: u16) -> Rig {
        let dir = crate::test_support::unique_tmp("slide_state");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.as_path().to_path_buf(), Config::default()).unwrap();
        let name = if doc.slides.is_empty() {
            "d.docx"
        } else {
            "d.pptx"
        };
        app.tab.preview_kind = Some(PreviewKind::Document(dir.as_path().join(name)));
        app.set_document(Some(Box::new(LoadedDocument::from_document(doc))));
        app.tab.md_raw = true;
        assert!(app.write_document_raw());
        app.setup_windowed();
        app.tab.preview_viewport = vh;
        assert!(app.is_windowed());
        Rig { app, _dir: dir }
    }

    fn deck_rig(n: usize, vh: u16) -> Rig {
        raw_rig(
            Document {
                markdown: deck_markdown(n),
                slides: slides(n),
                ..Document::default()
            },
            vh,
        )
    }

    // --- what the worker hands over -----------------------------------------------------------

    #[test]
    fn the_slide_lines_are_the_level_two_heading_lines_and_nothing_else() {
        let md = "# Title\n\n## Slide 1: a\n\ntext\n##hashtag\n  ## indented\n\n## Slide 2: b\n\nz\n### Slide 2.1\n";
        let d = LoadedDocument::from_document(Document {
            markdown: md.into(),
            slides: slides(2),
            ..Document::default()
        });
        // "##hashtag" and the indented line are not headings; "### " is level 3.
        assert_eq!(d.slide_lines, vec![2, 8]);
        assert_eq!(d.last_slide_byte, byte_of_line(md, 8));
        assert_eq!(&md[d.last_slide_byte as usize..][..10], "## Slide 2");
    }

    #[test]
    fn the_last_slide_byte_counts_bytes_not_characters() {
        // Multi-byte text before the last heading: the offset is in bytes (the reader's unit).
        let md = "## スライド 1: 日本語のタイトル\n\n本文です\n\n## スライド 2: 終わり\n";
        let d = LoadedDocument::from_document(Document {
            markdown: md.into(),
            slides: slides(2),
            ..Document::default()
        });
        assert_eq!(d.slide_lines, vec![0, 4]);
        assert_eq!(
            d.last_slide_byte as usize,
            md.find("## スライド 2").unwrap()
        );
        assert!(d.last_slide_byte as usize > md[..d.last_slide_byte as usize].chars().count());
    }

    #[test]
    fn a_first_slide_heading_is_at_byte_zero_and_one_slide_is_its_own_last() {
        let md = "## Slide 1: only\n\ntext\n";
        let d = LoadedDocument::from_document(Document {
            markdown: md.into(),
            slides: slides(1),
            ..Document::default()
        });
        assert_eq!((d.slide_lines.clone(), d.last_slide_byte), (vec![0], 0));
    }

    #[test]
    fn a_word_document_keeps_no_slide_lines_whatever_its_headings() {
        let md = "## Chapter 1\n\ntext\n\n## Chapter 2\n";
        let d = LoadedDocument::from_document(Document {
            markdown: md.into(),
            ..Document::default()
        });
        assert!(d.slide_lines.is_empty());
        assert_eq!(d.last_slide_byte, 0);
    }

    // --- the raw view's furthest top ----------------------------------------------------------

    #[test]
    fn the_raw_window_may_reach_the_last_slide_past_its_last_page() {
        // 3 slides x 20 lines = 60 lines; the last heading is line 40.
        let md = deck_markdown(3);
        let h40 = byte_of_line(&md, 40);
        // A 30-row page ends at line 30: the heading (line 40) lies past it.
        let mut r = deck_rig(3, 30);
        assert_eq!(r.app.win_max_top(30), Some((h40, Some(40))));
        // A 20-row page starts exactly at the heading: the ordinary last page (no widened line).
        assert_eq!(r.app.win_max_top(20), Some((h40, None)));
        // A 10-row page starts at line 50, past the heading: the last page.
        assert_eq!(r.app.win_max_top(10), Some((byte_of_line(&md, 50), None)));
        assert_eq!(r.app.slide_raw_floor(), Some((h40, 40)));
    }

    #[test]
    fn g_in_the_raw_view_puts_the_last_slide_at_the_top() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        r.app.preview_to_bottom();
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
        // The caret goes to the last line of the text, as for any text.
        assert_eq!(r.app.tab.preview_cursor_line, 59);
        assert_eq!(r.app.slide_position(), Some((3, 3)));
    }

    #[test]
    fn scrolling_down_stops_at_the_last_slide_and_knows_its_line() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        r.app.win_scroll_lines(500);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
        // And it stays there.
        r.app.win_scroll_lines(5);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
        // One line short of the end of the range: moves on, one line at a time.
        let mut s = deck_rig(3, 30);
        s.app.win_scroll_lines(39);
        assert_eq!(s.app.tab.preview_top_line, 39);
        s.app.win_scroll_lines(1);
        assert_eq!(s.app.tab.preview_top_line, 40);
        assert_eq!(s.app.tab.preview_byte_top, byte_of_line(&md, 40));
    }

    #[test]
    fn a_window_put_past_the_range_is_pulled_back_to_the_last_slide() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        // As after a resize: the top is further down than the range allows.
        r.app.tab.preview_byte_top = byte_of_line(&md, 55);
        r.app.tab.preview_top_line = 55;
        let _ = r.app.windowed_lines(30, 80);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
        // Exactly at the end of the range is not past it: nothing moves.
        let _ = r.app.windowed_lines(30, 80);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
    }

    #[test]
    fn the_scroll_bar_range_ends_at_the_last_slide() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        let x = r.app.window_scroll_extent(30).unwrap();
        assert_eq!(x.max, byte_of_line(&md, 40));
        assert_eq!(x.viewport, md.len() as u64 - byte_of_line(&md, 40));
    }

    #[test]
    fn a_word_document_in_the_raw_view_keeps_its_last_page_whatever_its_headings() {
        let md = deck_markdown(3);
        let mut r = raw_rig(
            Document {
                markdown: md.clone(),
                ..Document::default()
            },
            30,
        );
        assert_eq!(r.app.slide_raw_floor(), None);
        assert_eq!(r.app.win_max_top(30), Some((byte_of_line(&md, 30), None)));
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        r.app.preview_to_bottom();
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 30));
        assert_eq!(r.app.tab.preview_top_line, 30);
    }

    #[test]
    fn the_floor_exists_only_in_the_raw_view_of_a_consistent_deck() {
        // Not the raw view: no floor (the decorated view has its own limit).
        let mut r = deck_rig(3, 30);
        r.app.tab.md_raw = false;
        assert_eq!(r.app.slide_raw_floor(), None);
        // Not a document at all.
        let mut r = deck_rig(3, 30);
        r.app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("a.txt")));
        assert_eq!(r.app.slide_raw_floor(), None);
        // The deck says 3 slides but its Markdown has 4 level-2 headings: nothing is offered.
        let mut md = deck_markdown(3);
        md.push_str("## One too many\n");
        let r = raw_rig(
            Document {
                markdown: md,
                slides: slides(3),
                ..Document::default()
            },
            30,
        );
        assert_eq!(r.app.slide_raw_floor(), None);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        // ... and one too few.
        let r = raw_rig(
            Document {
                markdown: deck_markdown(2),
                slides: slides(3),
                ..Document::default()
            },
            30,
        );
        assert_eq!(r.app.slide_raw_floor(), None);
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
    }

    // --- the slide the raw view is on, and J / K ----------------------------------------------

    #[test]
    fn the_raw_view_is_on_the_last_heading_at_or_above_its_top_line() {
        let mut r = deck_rig(3, 30);
        for (top, want) in [
            (0, (1, 3)),
            (19, (1, 3)),
            (20, (2, 3)),
            (39, (2, 3)),
            (40, (3, 3)),
            (59, (3, 3)),
        ] {
            r.app.tab.preview_top_line = top;
            // The caret and the scroll position are somewhere else: only the top line counts.
            r.app.tab.preview_cursor_line = 59 - top;
            r.app.tab.preview_scroll = 7;
            assert_eq!(r.app.slide_position(), Some(want), "top line {top}");
        }
    }

    #[test]
    fn a_window_that_is_not_open_offers_nothing() {
        let mut r = deck_rig(3, 30);
        assert!(r.app.slide_can_turn());
        r.app.preview_win = None;
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        let line = r.app.tab.preview_top_line;
        r.app.slide_turn(1);
        assert_eq!(r.app.tab.preview_top_line, line);
    }

    #[test]
    fn j_and_k_in_the_raw_view_move_the_window_the_line_and_the_caret() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        let at = |r: &mut Rig, dir: i32, line: usize| {
            r.app.slide_turn(dir);
            assert_eq!(r.app.tab.preview_top_line, line, "top line");
            assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, line), "byte");
            assert_eq!(r.app.tab.preview_cursor_line, line, "caret");
        };
        at(&mut r, 1, 20);
        at(&mut r, 1, 40);
        // Last slide: J does nothing.
        at(&mut r, 1, 40);
        at(&mut r, -1, 20);
        at(&mut r, -1, 0);
        at(&mut r, -1, 0);
        // Inside a slide: K goes to its start, J to the next one.
        r.app.tab.preview_top_line = 25;
        r.app.tab.preview_byte_top = byte_of_line(&md, 25);
        at(&mut r, -1, 20);
        r.app.tab.preview_top_line = 25;
        r.app.tab.preview_byte_top = byte_of_line(&md, 25);
        at(&mut r, 1, 40);
    }

    #[test]
    fn a_one_slide_deck_has_a_position_but_no_turning() {
        let mut r = raw_rig(
            Document {
                markdown: deck_markdown(1),
                slides: slides(1),
                ..Document::default()
            },
            30,
        );
        assert_eq!(r.app.slide_position(), Some((1, 1)));
        assert!(!r.app.slide_can_turn());
        r.app.slide_turn(1);
        r.app.slide_turn(-1);
        assert_eq!(r.app.tab.preview_top_line, 0);
        assert_eq!(r.app.tab.preview_byte_top, 0);
    }

    #[test]
    fn a_two_slide_deck_can_turn() {
        let r = deck_rig(2, 30);
        assert!(r.app.slide_can_turn());
        assert_eq!(r.app.slide_position(), Some((1, 2)));
    }

    #[test]
    fn no_document_shown_means_no_slides() {
        let dir = crate::test_support::unique_tmp("slide_none");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.as_path().to_path_buf(), Config::default()).unwrap();
        assert_eq!(app.slide_position(), None);
        assert!(!app.slide_can_turn());
        assert_eq!(app.slide_raw_floor(), None);
        assert_eq!(app.slide_scroll_limit(100, 30), 70);
        app.slide_turn(1);
        // A deck that is held but is not what the tab shows.
        let mut r = deck_rig(3, 30);
        r.app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("a.txt")));
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        r.app.tab.md_raw = false;
        assert_eq!(r.app.slide_scroll_limit(100, 30), 70);
    }

    #[test]
    fn the_decorated_limit_is_the_plain_one_without_a_layout() {
        // Decorated view of a deck whose layout is not built yet: nothing to widen.
        let mut r = deck_rig(3, 30);
        r.app.tab.md_raw = false;
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
        app.tab.preview_kind = Some(PreviewKind::Document(path.clone()));
        app.tab.preview_path = Some(path);
        app.set_document(Some(Box::new(LoadedDocument::from_document(Document {
            markdown: md,
            slides: slides(n),
            ..Document::default()
        }))));
        app.tab.preview_viewport = vh;
        app.ensure_md_cache(w);
        assert!(app.md_cache.is_some());
        Rig { app, _dir: dir }
    }

    /// The display row of each slide's heading, from the layout.
    fn head_rows(r: &Rig) -> Vec<usize> {
        let c = r.app.md_cache.as_ref().unwrap();
        r.app
            .slide_head_lines(c)
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
        let lines = r.app.slide_head_lines(c).unwrap();
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
        let (heads, top) = r.app.slide_view().unwrap();
        assert_eq!(heads, rows);
        assert_eq!(top, 0);
    }

    #[test]
    fn j_and_k_in_the_decorated_view_scroll_to_the_heading_row() {
        let mut r = decorated_rig(4, 40, 10);
        let rows = head_rows(&r);
        for (k, row) in rows.iter().enumerate().skip(1) {
            r.app.slide_turn(1);
            assert_eq!(r.app.tab.preview_scroll as usize, *row, "J to {k}");
            assert_eq!(r.app.slide_position(), Some((k + 1, 4)));
        }
        r.app.slide_turn(1);
        assert_eq!(r.app.tab.preview_scroll as usize, rows[3], "the last stays");
        for k in (0..3).rev() {
            r.app.slide_turn(-1);
            assert_eq!(r.app.tab.preview_scroll as usize, rows[k], "K to {k}");
        }
        r.app.slide_turn(-1);
        assert_eq!(r.app.tab.preview_scroll, 0);
        // A top that is not on a heading: K goes to the start of the slide it is in.
        r.app.tab.preview_scroll = (rows[2] + 1) as u16;
        assert_eq!(r.app.slide_position(), Some((3, 4)));
        r.app.slide_turn(-1);
        assert_eq!(r.app.tab.preview_scroll as usize, rows[2]);
        r.app.tab.preview_scroll = (rows[2] + 1) as u16;
        r.app.slide_turn(1);
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
        r.app.document.as_mut().unwrap().slides.clear();
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
            let d = r.app.document.as_mut().unwrap();
            if delta > 0 {
                d.slides.push(SlideInfo {
                    number: 5,
                    title: "T5".into(),
                    hidden: false,
                });
            } else {
                d.slides.pop();
            }
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
            r.app.slide_turn(1);
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
        let (heads, _) = r.app.slide_view().unwrap();
        assert_eq!(heads.len(), 3);
        r.app.slide_turn(1);
        assert_eq!(r.app.slide_position(), Some((1, 3)));
        r.app.slide_turn(1);
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
        r.app.slide_turn(1);
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
        app.tab.preview_kind = Some(PreviewKind::Document(path.clone()));
        app.tab.preview_path = Some(path);
        let md = deck_markdown(3);
        app.set_document(Some(Box::new(LoadedDocument::from_document(Document {
            markdown: md,
            slides: slides(3),
            ..Document::default()
        }))));
        app.ensure_md_cache(60);
        let c = app.md_cache.as_ref().unwrap();
        let lines = app.slide_head_lines(c).unwrap();
        let (heads, _) = app.slide_view().unwrap();
        assert_eq!(heads, lines, "one row per line");
        app.slide_turn(1);
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

    #[test]
    fn the_decorated_limit_is_not_used_in_the_raw_view() {
        // The raw view has its own floor; a layout left over from the decorated view must not
        // widen anything.
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
        assert!(r.app.slide_scroll_limit(total, total + 50) > 0);
        r.app.tab.md_raw = true;
        assert_eq!(r.app.slide_scroll_limit(total, total + 50), 0);
        assert_eq!(r.app.slide_scroll_limit(total, 5), total - 5);
    }

    #[test]
    fn j_and_k_do_nothing_where_the_keys_are_not_offered() {
        // One slide and some text before its heading: the view is before the first heading, so J
        // would have somewhere to go - but the key is not offered for a single slide.
        let mut r = raw_rig(
            Document {
                markdown: "intro\n\n## Slide 1: only\n\nbody\n".into(),
                slides: slides(1),
                ..Document::default()
            },
            30,
        );
        assert_eq!(r.app.slide_position(), None);
        assert!(!r.app.slide_can_turn());
        r.app.slide_turn(1);
        assert_eq!(
            (r.app.tab.preview_top_line, r.app.tab.preview_byte_top),
            (0, 0)
        );
        r.app.slide_turn(-1);
        assert_eq!(
            (r.app.tab.preview_top_line, r.app.tab.preview_byte_top),
            (0, 0)
        );
        // Not a deck at all: nothing happens either.
        let mut w = raw_rig(
            Document {
                markdown: deck_markdown(3),
                ..Document::default()
            },
            30,
        );
        w.app.slide_turn(1);
        assert_eq!(w.app.tab.preview_top_line, 0);
    }

    #[test]
    fn the_last_slide_is_the_end_of_the_raw_range_with_line_numbers_on_too() {
        // With line numbers on, the total line count is known: it must not replace the line of the
        // last slide's heading.
        let md = deck_markdown(3);
        for g_key in [true, false] {
            let mut r = deck_rig(3, 30);
            r.app.cfg.ui.line_numbers = true;
            if g_key {
                r.app.preview_to_bottom();
            } else {
                r.app.win_scroll_lines(500);
            }
            assert_eq!(
                r.app.tab.preview_byte_top,
                byte_of_line(&md, 40),
                "g={g_key}"
            );
            assert_eq!(r.app.tab.preview_top_line, 40, "g={g_key}");
            // The ordinary last page keeps the count-based line.
            let mut w = raw_rig(
                Document {
                    markdown: md.clone(),
                    ..Document::default()
                },
                30,
            );
            w.app.cfg.ui.line_numbers = true;
            w.app.win_scroll_lines(500);
            assert_eq!(w.app.tab.preview_top_line, 30, "word, g={g_key}");
        }
    }

    #[test]
    fn a_window_exactly_at_the_end_of_the_range_is_left_alone() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        r.app.tab.preview_byte_top = byte_of_line(&md, 40);
        r.app.tab.preview_top_line = 7; // not the true line: a clamp would correct it
        let _ = r.app.windowed_lines(30, 80);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 7);
    }

    #[test]
    fn reaching_the_end_of_the_range_exactly_takes_the_line_of_the_last_slide() {
        let md = deck_markdown(3);
        let mut r = deck_rig(3, 30);
        // One line short of the end of the range, with a line number that is not the true one:
        // arriving at the end corrects it from the known line of the last heading.
        r.app.tab.preview_byte_top = byte_of_line(&md, 39);
        r.app.tab.preview_top_line = 7;
        r.app.win_scroll_lines(1);
        assert_eq!(r.app.tab.preview_byte_top, byte_of_line(&md, 40));
        assert_eq!(r.app.tab.preview_top_line, 40);
    }
}
