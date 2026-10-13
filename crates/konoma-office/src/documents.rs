//! The `document` (Word) and `presentation` (slides) plugins.
//!
//! Both read the file into a Markdown text with its pictures (the reading is in
//! `konoma-office-core`) and hand it to the host as a **document**:
//!
//! - **Word** (docx, docm, dotx, dotm, odt, ott): one representation, returned as a
//!   [`View::Reopen`], "open this again as Markdown, converted from the file". The host draws it
//!   with its document widget (it cannot be written back; `R` shows the converted text).
//! - **Slides** (pptx, pptm, ppsx, ppsm, potx, potm, odp, otp): a [`View::Document`] whose pages are
//!   the slides ([`PageLayout::Anchored`]: one long document, a slide starts at each level-2
//!   heading) and, when the reader drew the slides, two representations: `picture` (each slide's
//!   drawing between its heading and its notes) and `text` (the converted Markdown). The picture
//!   of a slide is a [`Resource::Lazy`]: the host asks for it when it scrolls into view, at the
//!   size of the frame it is placed in, and can cancel the drawing.
//!
//! **Memory.** The pictures of the file are held once, as `Arc<[u8]>`, and shared with the host
//! (never copied for a view). A Word session hands everything over with its one view and keeps
//! nothing; a presentation session keeps them to make its other representation. A slide's scene is drawn to an SVG only when the host
//! asks, and the SVG is handed back as untrusted (the host rasterizes it in a child process).
//!
//! **Work group.** The plugins read in the `office` group with the spreadsheet: one read at a
//! time across the group, the newest request waiting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use konoma_office_core::docx::pptx::load_presentation_cancellable;
use konoma_office_core::docx::{load_document_cancellable, DocOptions as ReadOptions, Document};
use konoma_office_core::slide_draw::{render_svg_cancellable, SlideScene};
use konoma_office_core::{Cancel as ReadCancel, OfficeError};
use konoma_plugin::input::{Origin, Resource, Resources};
use konoma_plugin::view::{
    DocOptions, DocumentView, ImageData, Layout, PageLayout, Reopen, ReprDecl, View, ViewReq,
};
use konoma_plugin::{
    t, Cancel, Failure, Input, LazyResource, Manifest, OpenCx, Plugin, ResourceReq, Session, ViewCx,
};

use crate::texts;

/// Which kind of file a session reads: it decides the reader and the words of a failure
/// (`[document]` / `[presentation]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Word,
    Deck,
}

/// The id of the representation that shows each slide as a picture.
pub const REPR_PICTURE: &str = "picture";
/// The id of the representation that shows the slides as the converted Markdown.
pub const REPR_TEXT: &str = "text";
/// The heading level a slide starts at in both representations.
pub const SLIDE_HEADING_LEVEL: u8 = 2;

/// Pixels per EMU: slide pictures are SVGs at 96 dpi (1 px = 9525 EMU).
const EMU_PER_PX: f64 = 9525.0;

/// The pixel size of a slide drawn from `scene`; `None` for a size that is not a positive, finite
/// number of pixels (such a slide has nothing to size a box by and is drawn as its alt text).
pub fn slide_px(scene: &SlideScene) -> Option<(u32, u32)> {
    let side = |emu: f64| {
        let px = (emu / EMU_PER_PX).round();
        (px.is_finite() && px >= 1.0).then(|| px.min(u32::MAX as f64) as u32)
    };
    Some((side(scene.width)?, side(scene.height)?))
}

/// One slide's drawing, made when the host asks for it.
struct SlideResource {
    scene: SlideScene,
    /// The pictures of the file, by the keys the scene refers to.
    media: Arc<HashMap<String, Arc<[u8]>>>,
    dims: Option<(u32, u32)>,
    /// The longest raster side the last render found drawable; 0 until the first render.
    max_raster: AtomicU32,
    /// A render left something out (a size budget of the writer).
    truncated: AtomicBool,
}

impl std::fmt::Debug for SlideResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlideResource")
            .field("dims", &self.dims)
            .finish_non_exhaustive()
    }
}

impl LazyResource for SlideResource {
    fn natural_size(&self) -> Option<(u32, u32)> {
        self.dims
    }

    fn render(&self, _req: &ResourceReq, cancel: &Cancel) -> Result<ImageData, Failure> {
        let drawn = render_svg_cancellable(
            &self.scene,
            // The drawing code wants shared owned bytes; a slide uses a few of the pictures and
            // the copy is transient (the SVG that embeds them is larger still).
            &|key| self.media.get(key).map(|b| Arc::new(b.to_vec())),
            &|| cancel.is_cancelled(),
        );
        // The preview moved on: nothing to show, nothing to remember.
        if drawn.cancelled {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        if drawn.truncated {
            self.truncated.store(true, Ordering::Relaxed);
        }
        self.max_raster
            .store(drawn.max_raster_px(), Ordering::Relaxed);
        Ok(ImageData::Svg {
            data: Arc::from(drawn.svg.into_bytes()),
            base_dir: None,
            // The slide's pictures come from the file: the host draws it in its supervised child
            // process like any other untrusted SVG.
            trusted: false,
        })
    }

    fn max_raster_px(&self) -> Option<u32> {
        Some(self.max_raster.load(Ordering::Relaxed)).filter(|px| *px > 0)
    }

    fn truncated(&self) -> bool {
        self.truncated.load(Ordering::Relaxed)
    }
}

/// One slide's drawing as a resource over `scene`, whose pictures are looked up in `media`.
/// Made on demand like the slides of a converted presentation (for tests of the host's drawing
/// rules).
#[doc(hidden)]
pub fn slide_resource(
    scene: SlideScene,
    media: HashMap<String, Arc<[u8]>>,
) -> Arc<dyn LazyResource> {
    let dims = slide_px(&scene);
    Arc::new(SlideResource {
        scene,
        media: Arc::new(media),
        dims,
        max_raster: AtomicU32::new(0),
        truncated: AtomicBool::new(false),
    })
}

/// A converted file: what the readers produced, in the form the views are made from.
pub struct Converted {
    markdown: String,
    /// A presentation's picture view: the same headings with the slide pictures between them.
    /// Empty for a Word document and for a deck without drawings.
    picture_markdown: String,
    truncated: bool,
    /// The pictures of the file, by the keys the Markdown refers to.
    images: Vec<(String, Arc<[u8]>)>,
    /// The slides' pictures, by their keys (empty when the deck has no picture view).
    slide_resources: Vec<(String, Arc<SlideResource>)>,
    /// How many slides a presentation has (0 for a Word document).
    slides: usize,
}

impl Converted {
    /// Builds the views' material from what a reader produced.
    pub fn from_document(doc: Document) -> Converted {
        let mut images: Vec<(String, Arc<[u8]>)> = Vec::with_capacity(doc.images.len());
        for im in doc.images {
            images.push((im.key, Arc::from(im.bytes)));
        }
        // The picture view needs one scene and one key per slide; anything else is a reader that
        // did not (fully) fill them, and the deck then has its text view only.
        let consistent = !doc.slides.is_empty()
            && doc.slide_scenes.len() == doc.slides.len()
            && doc.slide_keys.len() == doc.slides.len()
            && !doc.picture_markdown.is_empty();
        let (picture_markdown, slide_resources) = if consistent {
            let media: Arc<HashMap<String, Arc<[u8]>>> = Arc::new(images.iter().cloned().collect());
            let resources = doc
                .slide_keys
                .into_iter()
                .zip(doc.slide_scenes)
                .map(|(key, scene)| {
                    let dims = slide_px(&scene);
                    (
                        key,
                        Arc::new(SlideResource {
                            scene,
                            media: media.clone(),
                            dims,
                            max_raster: AtomicU32::new(0),
                            truncated: AtomicBool::new(false),
                        }),
                    )
                })
                .collect();
            (doc.picture_markdown, resources)
        } else {
            (String::new(), Vec::new())
        };
        Converted {
            markdown: doc.markdown,
            picture_markdown,
            truncated: doc.truncated,
            images,
            slide_resources,
            slides: doc.slides.len(),
        }
    }

    /// Whether the deck has a picture view.
    fn has_pictures(&self) -> bool {
        !self.picture_markdown.is_empty()
    }

    /// The layout of a presentation (a Word document has the single layout).
    fn layout(&self) -> Layout {
        if self.slides == 0 {
            return Layout::single();
        }
        let pages = Some(PageLayout::Anchored {
            heading_level: SLIDE_HEADING_LEVEL,
            count: self.slides,
        });
        if !self.has_pictures() {
            return Layout {
                pages,
                ..Layout::default()
            };
        }
        Layout {
            reprs: vec![
                ReprDecl::new(REPR_PICTURE, t!(en: "slides", ja: "スライド表示")),
                ReprDecl::new(REPR_TEXT, t!(en: "text view", ja: "文字表示")),
            ],
            default_repr: Some(REPR_PICTURE.to_string()),
            default_repr_without_pixels: Some(REPR_TEXT.to_string()),
            pages,
        }
    }

    /// The pictures of the file as resources (shared, not copied).
    fn image_resources(&self) -> Resources {
        let mut r = Resources::new();
        for (key, bytes) in &self.images {
            r.insert(key.clone(), Resource::Bytes(bytes.clone()));
        }
        r
    }

    /// The Word document as a "converted Markdown" reopen. Everything moves into the view: the
    /// session keeps nothing of the document, so the pictures live exactly as long as the
    /// host's document does.
    fn into_reopen(self, from: &Path) -> View {
        let resources = self.image_resources();
        View::Reopen(Reopen {
            content_type: "markdown".to_string(),
            text: self.markdown,
            resources,
            origin: Origin::Converted {
                from: from.to_path_buf(),
            },
            truncated: self.truncated,
        })
    }

    /// The presentation in the representation `repr` (`None` = the default for the terminal).
    fn deck_view(&self, repr: Option<&str>, pixels: bool, from: &Path) -> View {
        let picture = self.has_pictures()
            && match repr {
                Some(REPR_PICTURE) => true,
                Some(REPR_TEXT) => false,
                _ => pixels,
            };
        let (markdown, resources) = if picture {
            let mut r = self.image_resources();
            for (key, slide) in &self.slide_resources {
                r.insert(key.clone(), Resource::Lazy(slide.clone()));
            }
            (self.picture_markdown.clone(), r)
        } else {
            (self.markdown.clone(), self.image_resources())
        };
        View::Document(DocumentView {
            markdown,
            origin: Origin::Converted {
                from: from.to_path_buf(),
            },
            resources,
            options: DocOptions {
                front_matter: false,
                tasks_writable: false,
                ..DocOptions::default()
            },
            title: None,
            truncated: self.truncated,
        })
    }
}

/// An old binary Office file (`.doc`, OLE/CFB) opened as a `.docx` is not a damaged zip, but when
/// its compound-file structure cannot be read the zip reader reports it as `Corrupt` (a valid one
/// is already `Unsupported`), which says "damaged". Looks at the first 8 bytes (the CFB signature)
/// and reports `Unsupported` ("not a Word format konoma reads, an old .doc for one") instead.
/// Every other error is returned as it came.
fn legacy_binary_reason(path: &Path, err: OfficeError) -> OfficeError {
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

/// Reads `path` as a Word document (`kind` = Word) or a presentation, stopping early once `cancel`
/// says so. A panic in a reader (they sit on third-party parsers) becomes a "corrupt" reason.
pub fn read_converted(
    path: &Path,
    kind: DocKind,
    lang: &str,
    cancel: &ReadCancel,
) -> Result<Converted, OfficeError> {
    let opts = ReadOptions {
        labels: texts::slide_labels(lang),
        ..ReadOptions::default()
    };
    let loaded = konoma_base::panic::catch_silent(|| match kind {
        DocKind::Deck => load_presentation_cancellable(path, &opts, Some(cancel)),
        DocKind::Word => load_document_cancellable(path, &opts, Some(cancel)),
    })
    .unwrap_or_else(|| Err(OfficeError::Corrupt("reader panicked".into())));
    match loaded {
        Ok(doc) => Ok(Converted::from_document(doc)),
        Err(e) => Err(legacy_binary_reason(path, e)),
    }
}

/// An opened file. `open` only checks it was handed a file; the first `view` reads it.
pub struct OfficeSession {
    kind: DocKind,
    path: PathBuf,
    converted: Option<Converted>,
}

impl OfficeSession {
    /// A session over an already converted file (tests).
    pub fn from_converted(kind: DocKind, path: PathBuf, converted: Converted) -> OfficeSession {
        OfficeSession {
            kind,
            path,
            converted: Some(converted),
        }
    }
}

impl Session for OfficeSession {
    fn layout(&self) -> Layout {
        self.converted
            .as_ref()
            .map_or_else(Layout::single, Converted::layout)
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if self.converted.is_none() {
            let host = cx.cancel.clone();
            let cancel = ReadCancel::new(move || host.is_cancelled());
            let read = read_converted(&self.path, self.kind, cx.lang, &cancel);
            if cx.cancel.is_cancelled() {
                // What was read is partial and the host is not waiting for it.
                return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
            }
            match read {
                Ok(c) => self.converted = Some(c),
                Err(e) => return Err(texts::doc_failure_of(&e, self.kind)),
            }
        }
        let nothing =
            || texts::doc_failure_of(&OfficeError::Corrupt("nothing was read".into()), self.kind);
        Ok(match self.kind {
            // A Word document is returned once and not kept (a second `view` reads it again).
            DocKind::Word => self
                .converted
                .take()
                .ok_or_else(nothing)?
                .into_reopen(&self.path),
            DocKind::Deck => self.converted.as_ref().ok_or_else(nothing)?.deck_view(
                req.repr.as_deref(),
                cx.term.pixels,
                &self.path,
            ),
        })
    }
}

fn open_session(kind: DocKind, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
    let Input::File { path, .. } = input else {
        return Err(Failure::unsupported(t!(
            en: "this plugin opens files only",
            ja: "このプラグインはファイルだけを開きます",
        )));
    };
    if cx.cancel.is_cancelled() {
        return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
    }
    Ok(Box::new(OfficeSession {
        kind,
        path,
        converted: None,
    }))
}

/// The Word document plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct DocumentPlugin;

impl Plugin for DocumentPlugin {
    fn manifest(&self) -> Manifest {
        crate::document_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        open_session(DocKind::Word, input, cx)
    }
}

/// The presentation plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct PresentationPlugin;

impl Plugin for PresentationPlugin {
    fn manifest(&self) -> Manifest {
        crate::presentation_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        open_session(DocKind::Deck, input, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_office_core::docx::pptx::SlideInfo;
    use konoma_office_core::docx::DocImage;
    use konoma_office_core::slide_draw::{Fill, Rgba};
    use konoma_plugin::testing::check_manifest;
    use konoma_plugin::view::PageLayout;
    use konoma_plugin::{FailureKind, Settings, TermCaps};
    use konoma_test_support::{repo_root, unique_tmp};

    fn scene_of(w: f64, h: f64) -> SlideScene {
        SlideScene {
            width: w,
            height: h,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items: Vec::new(),
            truncated: false,
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

    /// A converted deck of `n` slides with `scenes` scenes and `keys` picture keys.
    fn deck(n: usize, scenes: usize, keys: usize, picture_md: &str) -> Document {
        Document {
            markdown: "## Slide 1: T1\n".into(),
            slides: slides(n),
            slide_scenes: (0..scenes)
                .map(|_| scene_of(9_144_000.0, 5_143_500.0))
                .collect(),
            slide_keys: (1..=keys)
                .map(|k| format!("office-img://aaaaaaaaaaaa/slide-{k}.svg"))
                .collect(),
            picture_markdown: picture_md.into(),
            ..Document::default()
        }
    }

    fn deck_session(doc: Document) -> OfficeSession {
        OfficeSession::from_converted(
            DocKind::Deck,
            PathBuf::from("d.pptx"),
            Converted::from_document(doc),
        )
    }

    fn view_of(
        session: &mut OfficeSession,
        repr: Option<&str>,
        pixels: bool,
    ) -> Result<View, Failure> {
        let settings = Settings::empty();
        let req = ViewReq {
            repr: repr.map(str::to_string),
            ..ViewReq::default()
        };
        session.view(
            &req,
            &ViewCx::new(settings, "en").with_term(TermCaps { pixels }),
        )
    }

    fn document_of(view: View) -> DocumentView {
        match view {
            View::Document(d) => d,
            other => panic!("expected a document view: {other:?}"),
        }
    }

    #[test]
    fn the_manifests_are_clean_bind_their_globs_and_join_the_office_group() {
        for (plugin, id, glob) in [
            (
                &DocumentPlugin as &dyn Plugin,
                crate::DOCUMENT_ID,
                crate::DOCUMENT_GLOB,
            ),
            (
                &PresentationPlugin as &dyn Plugin,
                crate::PRESENTATION_ID,
                crate::PRESENTATION_GLOB,
            ),
        ] {
            assert!(check_manifest(plugin).is_empty(), "{id}");
            let m = plugin.manifest();
            assert_eq!(m.id, id);
            assert_eq!(m.work_group.as_deref(), Some(crate::OFFICE_WORK_GROUP));
            assert!(
                format!("{:?}", m.binds).contains(glob),
                "{id} binds {glob}: {:?}",
                m.binds
            );
            assert_eq!(m.verbs.len(), 2, "{id}: e and o");
        }
    }

    #[test]
    fn a_slide_is_sized_in_pixels_from_emu_and_clamped_not_wrapped() {
        assert_eq!(
            slide_px(&scene_of(9_144_000.0, 5_143_500.0)),
            Some((960, 540))
        );
        assert_eq!(
            slide_px(&scene_of(9_144_000.0, 6_858_000.0)),
            Some((960, 720))
        );
        assert_eq!(slide_px(&scene_of(0.0, 5_143_500.0)), None);
        assert_eq!(slide_px(&scene_of(f64::NAN, 5_143_500.0)), None);
        assert_eq!(slide_px(&scene_of(1e30, 1e30)), Some((u32::MAX, u32::MAX)));
    }

    #[test]
    fn a_consistent_deck_has_both_representations_and_anchored_pages() {
        let c = Converted::from_document(deck(3, 3, 3, "## Slide 1: T1\n\n![a](x)\n"));
        let layout = c.layout();
        assert_eq!(
            layout.pages,
            Some(PageLayout::Anchored {
                heading_level: SLIDE_HEADING_LEVEL,
                count: 3
            })
        );
        let ids: Vec<_> = layout.reprs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [REPR_PICTURE, REPR_TEXT]);
        assert_eq!(layout.default_repr.as_deref(), Some(REPR_PICTURE));
        assert_eq!(
            layout.default_repr_without_pixels.as_deref(),
            Some(REPR_TEXT)
        );
    }

    #[test]
    fn a_deck_whose_reader_did_not_fill_everything_has_its_text_only() {
        for (n, scenes, keys, md) in [
            (3, 2, 3, "x"),
            (3, 3, 2, "x"),
            (3, 4, 3, "x"),
            (3, 3, 3, ""),
            (3, 0, 0, "x"),
            (0, 3, 3, "x"),
        ] {
            let c = Converted::from_document(deck(n, scenes, keys, md));
            assert!(c.slide_resources.is_empty(), "{n} {scenes} {keys} {md:?}");
            assert!(c.picture_markdown.is_empty(), "{n} {scenes} {keys} {md:?}");
            assert!(c.layout().reprs.is_empty());
            // Asked for pictures, the text is what comes back.
            let mut s = OfficeSession::from_converted(DocKind::Deck, "d.pptx".into(), c);
            let d = document_of(view_of(&mut s, Some(REPR_PICTURE), true).unwrap());
            assert_eq!(d.markdown, "## Slide 1: T1\n");
        }
        // Not a deck at all: no pages.
        assert_eq!(
            Converted::from_document(deck(0, 3, 3, "x")).layout(),
            Layout::single()
        );
    }

    #[test]
    fn the_default_representation_follows_the_terminal_and_a_named_one_wins() {
        let mut s = deck_session(deck(2, 2, 2, "PICTURES\n"));
        let pic = |s: &mut OfficeSession, repr, pixels| {
            document_of(view_of(s, repr, pixels).unwrap()).markdown
        };
        assert_eq!(
            pic(&mut s, None, true),
            "PICTURES\n",
            "pixels: the pictures"
        );
        assert_eq!(
            pic(&mut s, None, false),
            "## Slide 1: T1\n",
            "half blocks: the text"
        );
        assert_eq!(pic(&mut s, Some(REPR_TEXT), true), "## Slide 1: T1\n");
        assert_eq!(pic(&mut s, Some(REPR_PICTURE), false), "PICTURES\n");
    }

    #[test]
    fn the_slides_are_lazy_resources_and_the_text_view_holds_none() {
        let mut s = deck_session(deck(2, 2, 2, "PICTURES\n"));
        let d = document_of(view_of(&mut s, Some(REPR_PICTURE), true).unwrap());
        assert_eq!(d.resources.len(), 2);
        for (key, r) in d.resources.iter() {
            assert!(key.ends_with(".svg"));
            let Resource::Lazy(l) = r else {
                panic!("a slide is made on demand");
            };
            assert_eq!(l.natural_size(), Some((960, 540)));
            assert_eq!(l.max_raster_px(), None, "unknown until drawn");
        }
        let t = document_of(view_of(&mut s, Some(REPR_TEXT), true).unwrap());
        assert!(t.resources.is_empty());
        assert!(matches!(t.origin, Origin::Converted { .. }));
        assert!(!t.options.tasks_writable);
    }

    #[test]
    fn drawing_a_slide_gives_an_untrusted_svg_and_learns_its_raster_limit() {
        let c = Converted::from_document(deck(1, 1, 1, "P\n"));
        let (_, slide) = &c.slide_resources[0];
        let img = slide
            .render(&ResourceReq { max_px: 800 }, &Cancel::never())
            .unwrap();
        let ImageData::Svg { data, trusted, .. } = img else {
            panic!("a slide is an SVG");
        };
        assert!(!trusted, "the host rasterizes it in a child process");
        assert!(String::from_utf8_lossy(&data).contains("<svg"));
        assert!(slide.max_raster_px().is_some_and(|px| px > 0));
        assert!(!slide.truncated());
    }

    #[test]
    fn a_cancelled_slide_gives_up() {
        let c = Converted::from_document(deck(1, 1, 1, "P\n"));
        let (_, slide) = &c.slide_resources[0];
        let err = slide
            .render(&ResourceReq { max_px: 800 }, &Cancel::from_fn(|| true))
            .unwrap_err();
        assert_eq!(err.kind, FailureKind::Cancelled);
        assert_eq!(slide.max_raster_px(), None, "nothing learned from a cancel");
    }

    #[test]
    fn a_picture_of_the_file_is_held_once_and_shared_with_the_slides() {
        let mut doc = deck(2, 2, 2, "P\n");
        doc.images.push(DocImage {
            key: "office-img://aaaaaaaaaaaa/p.png".into(),
            bytes: vec![1, 2, 3],
            name: "p.png".into(),
        });
        let c = Converted::from_document(doc);
        let held = c.images[0].1.clone();
        let held = &held;
        for (_, slide) in &c.slide_resources {
            let got = slide.media.get("office-img://aaaaaaaaaaaa/p.png").unwrap();
            assert!(Arc::ptr_eq(got, held), "the slide's media share the bytes");
            assert!(slide
                .media
                .get("office-img://aaaaaaaaaaaa/slide-1.svg")
                .is_none());
        }
        // Both representations give the host the same bytes, not copies.
        let mut s = OfficeSession::from_converted(DocKind::Deck, "d.pptx".into(), c);
        for repr in [REPR_PICTURE, REPR_TEXT] {
            let d = document_of(view_of(&mut s, Some(repr), true).unwrap());
            let Some(Resource::Bytes(b)) = d.resources.get("office-img://aaaaaaaaaaaa/p.png")
            else {
                panic!("{repr}: the file's picture is bytes");
            };
            assert!(Arc::ptr_eq(b, held), "{repr}");
        }
    }

    #[test]
    fn a_word_document_is_a_converted_markdown_reopen_with_its_pictures() {
        let c = Converted::from_document(Document {
            markdown: "Hello\n".into(),
            truncated: true,
            images: vec![DocImage {
                key: "office-img://aaaaaaaaaaaa/p.png".into(),
                bytes: vec![9],
                name: "p.png".into(),
            }],
            ..Document::default()
        });
        assert_eq!(c.layout(), Layout::single());
        let mut s = OfficeSession::from_converted(DocKind::Word, "d.docx".into(), c);
        let View::Reopen(r) = view_of(&mut s, None, true).unwrap() else {
            panic!("Word is a reopen");
        };
        assert_eq!(r.content_type, "markdown");
        assert_eq!(r.text, "Hello\n");
        assert!(r.truncated);
        assert_eq!(r.resources.len(), 1);
        assert!(matches!(
            r.origin,
            Origin::Converted { from } if from == Path::new("d.docx")
        ));
    }

    // --- reading real files --------------------------------------------------------------

    fn testdata(name: &str) -> Option<PathBuf> {
        let p = repo_root().join("testdata/office").join(name);
        if p.exists() {
            Some(p)
        } else {
            eprintln!(
                "SKIP: testdata/office/{name} not found — this test verifies nothing this run"
            );
            None
        }
    }

    fn open(plugin: &dyn Plugin, path: &Path) -> Box<dyn Session> {
        konoma_plugin::testing::open_file(plugin, path).unwrap()
    }

    #[test]
    fn a_real_word_file_is_read_by_the_first_view_and_a_real_deck_has_its_pages() {
        if let Some(p) = testdata("word.docx") {
            let mut s = open(&DocumentPlugin, &p);
            assert_eq!(
                s.layout(),
                Layout::single(),
                "nothing is known before a read"
            );
            let View::Reopen(r) = s.view(&ViewReq::default(), &ViewCx::bare()).unwrap() else {
                panic!("Word is a reopen");
            };
            assert!(!r.text.is_empty());
        }
        if let Some(p) = testdata("slides.pptx") {
            let mut s = open(&PresentationPlugin, &p);
            let d = document_of(
                s.view(
                    &ViewReq {
                        repr: Some(REPR_TEXT.into()),
                        ..ViewReq::default()
                    },
                    &ViewCx::bare(),
                )
                .unwrap(),
            );
            assert!(d.markdown.contains("## "));
            let Some(PageLayout::Anchored {
                heading_level,
                count,
            }) = s.layout().pages
            else {
                panic!("a deck has anchored pages");
            };
            assert_eq!(heading_level, 2);
            assert_eq!(d.markdown.matches("\n## ").count() + 1, count);
        }
    }

    #[test]
    fn the_slide_words_follow_the_users_language() {
        let Some(p) = testdata("slides-ja.pptx") else {
            return;
        };
        let read = |lang: &str| {
            let mut s = open(&PresentationPlugin, &p);
            let settings = Settings::empty();
            document_of(
                s.view(
                    &ViewReq {
                        repr: Some(REPR_TEXT.into()),
                        ..ViewReq::default()
                    },
                    &ViewCx::new(settings, lang),
                )
                .unwrap(),
            )
            .markdown
        };
        assert!(read("ja").contains("## スライド 1"));
        assert!(read("en").contains("## Slide 1"));
    }

    fn failure_of_file(plugin: &dyn Plugin, bytes: &[u8], name: &str) -> Failure {
        let dir = unique_tmp("konoma_office_docs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.as_path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let mut s = open(plugin, &path);
        s.view(&ViewReq::default(), &ViewCx::bare()).unwrap_err()
    }

    #[test]
    fn files_that_cannot_be_read_say_why_in_the_words_of_their_kind() {
        let f = failure_of_file(&DocumentPlugin, b"not a zip", "bad.docx");
        assert_eq!(f.kind, FailureKind::Corrupt);
        assert!(
            f.message.en().starts_with("[document] "),
            "{}",
            f.message.en()
        );
        let f = failure_of_file(&PresentationPlugin, b"not a zip", "bad.pptx");
        assert_eq!(f.kind, FailureKind::Corrupt);
        assert!(f.message.en().starts_with("[presentation] "));
    }

    #[test]
    fn an_old_binary_file_is_not_a_format_konoma_reads_rather_than_damaged() {
        let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        cfb.extend_from_slice(&[0u8; 512]);
        let f = failure_of_file(&DocumentPlugin, &cfb, "old.docx");
        assert_eq!(f.kind, FailureKind::Unsupported, "{f}");
        assert!(f.message.en().contains("an old .doc"));
        let f = failure_of_file(&PresentationPlugin, &cfb, "old.ppt");
        assert_eq!(f.kind, FailureKind::Unsupported, "{f}");
        assert!(f.message.en().contains("an old .ppt"));
    }

    #[test]
    fn a_cancelled_read_gives_up_and_a_cancelled_open_does_not_start() {
        let Some(p) = testdata("word.docx") else {
            return;
        };
        let mut s = open(&DocumentPlugin, &p);
        let settings = Settings::empty();
        let cx = ViewCx::new(settings, "en").with_cancel(Cancel::from_fn(|| true));
        let err = s.view(&ViewReq::default(), &cx).unwrap_err();
        assert_eq!(err.kind, FailureKind::Cancelled);
        let open_cx = OpenCx::new(settings, "en").with_cancel(Cancel::from_fn(|| true));
        let err = DocumentPlugin
            .open(
                Input::File {
                    path: p,
                    hint: None,
                },
                &open_cx,
            )
            .err()
            .expect("a cancelled open does not start");
        assert_eq!(err.kind, FailureKind::Cancelled);
    }

    #[test]
    fn only_a_file_input_opens() {
        let err = DocumentPlugin
            .open(
                Input::Embed {
                    kind: "x".into(),
                    source: String::new(),
                    display: false,
                },
                &OpenCx::bare(),
            )
            .err()
            .expect("an embed is not a file");
        assert_eq!(err.kind, FailureKind::Unsupported);
    }
}
