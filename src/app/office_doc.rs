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
}

impl LoadedDocument {
    /// Builds the App-side form on the worker thread (sizes are read here, so the UI thread never
    /// parses an image header).
    pub(super) fn from_document(doc: crate::preview::office::docx::Document) -> Self {
        let pictures = doc
            .images
            .into_iter()
            .map(|im| {
                let dims = picture_dims(&im.bytes);
                (
                    im.key,
                    DocPicture {
                        bytes: Arc::new(im.bytes),
                        dims,
                    },
                )
            })
            .collect();
        LoadedDocument {
            markdown: doc.markdown,
            truncated: doc.truncated,
            pictures,
        }
    }
}

/// Pixel size of an in-memory picture without decoding it: raster formats first, then SVG (the same
/// order as `md_image_dims` for a file).
fn picture_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .or_else(|| crate::preview::svg::intrinsic_size_bytes(bytes))
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

    /// Pixel size of one picture of the open document.
    pub(super) fn document_picture_dims(&self, url: &str) -> Option<(u32, u32)> {
        self.document.as_ref()?.pictures.get(url)?.dims
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
