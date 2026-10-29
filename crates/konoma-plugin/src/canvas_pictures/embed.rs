//! Drawing an embedded kind (`Input::Embed`: a mermaid fence, a formula) into a picture,
//! host-neutral. konoma (a fence or a formula in a Markdown document) and the standalone app call
//! this one [`EmbedRender`], so the same plugin draws the same SVG the same way in either: the
//! SVG of a plugin whose origin the host trusts is drawn in this process, any other by the host's
//! supervised drawer ([`SvgDraw`]), which reads no file.
//!
//! What differs between the hosts is injected: the drawer ([`SvgDraw`], konoma's is a child process
//! of its own executable) and how a plugin id becomes settings and a trust decision
//! ([`embed_render_for`]).

use std::path::PathBuf;
use std::sync::Arc;

use konoma_widgets::inline_images::SvgTrust;

use super::{redraw_svg, SvgDraw};
use crate::registry::Registry;
use crate::view::{ImageData, ImageView};
use crate::{Input, OpenCx, Plugin, Settings, View, ViewCx, ViewReq};

/// The raster and the SVG it was made from, or the reason there is none.
pub type EmbedPicture = (Result<image::DynamicImage, String>, Option<Arc<Vec<u8>>>);

/// What a worker needs to ask an embed plugin for a picture (owned: it moves to a thread).
#[derive(Clone)]
pub struct EmbedRender {
    plugin: Arc<dyn Plugin>,
    settings: Settings,
    lang: String,
    /// Whether the host believes this plugin's `trusted` claim: it is a standard plugin that draws
    /// its own SVG. Decided by where the plugin comes from, never by what it says.
    trusted_origin: bool,
    /// Draws the SVGs that are not drawn in this process.
    svg: Arc<dyn SvgDraw + Send + Sync>,
}

impl EmbedRender {
    pub fn new(
        plugin: Arc<dyn Plugin>,
        settings: Settings,
        lang: impl Into<String>,
        trusted_origin: bool,
        svg: Arc<dyn SvgDraw + Send + Sync>,
    ) -> EmbedRender {
        EmbedRender {
            plugin,
            settings,
            lang: lang.into(),
            trusted_origin,
            svg,
        }
    }

    /// The plugin that draws the kind.
    pub fn plugin(&self) -> &Arc<dyn Plugin> {
        &self.plugin
    }

    /// The settings the plugin is opened with.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The language the plugin is opened with.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Open the plugin on `source` and rasterize the SVG its view holds at `max_px` (the longest
    /// side). `label` names the thing for the failure text; `base` is the file name the rasterizer
    /// resolves relative references from (the SVGs konoma draws have none).
    ///
    /// An SVG from a plugin whose origin the host trusts is drawn in this process, and only when
    /// the plugin vouches for it (`trusted`): the host keeps the SVG and rasterizes it again later
    /// (sharp zoom, rebuilding an evicted raster) in this process too. An SVG from any other
    /// plugin is drawn by the supervised drawer, with no file to read, and is drawn the same way
    /// every time it is made again (see [`EmbedRender::svg_trust`]).
    pub fn picture(
        &self,
        kind: &str,
        source: &str,
        display: bool,
        max_px: u32,
        label: &str,
        base: &str,
    ) -> EmbedPicture {
        let failed = || (Err(format!("{label} render failed")), None);
        let input = Input::Embed {
            kind: kind.to_string(),
            source: source.to_string(),
            display,
        };
        let open_cx = OpenCx::new(&self.settings, &self.lang);
        let Ok(mut session) = self.plugin.open(input, &open_cx) else {
            return failed();
        };
        let view_cx = ViewCx::new(&self.settings, &self.lang);
        let Ok(View::Image(ImageView {
            data: ImageData::Svg { data, trusted, .. },
            ..
        })) = session.view(&ViewReq::default(), &view_cx)
        else {
            return failed();
        };
        // The retained copy is what the host keeps for the layout and for later rasters.
        let svg = Arc::new(data.to_vec());
        // A plugin we trust that does not vouch for its own SVG is a bug in it: refuse, so an
        // entry is never half trusted.
        if self.trusted_origin && !trusted {
            return failed();
        }
        // The same function draws it again later (`redraw_svg`).
        let img = redraw_svg(&svg, &self.svg_trust(base), max_px, &*self.svg, &|| false)
            .map_err(|_| "rasterize failed".to_string());
        (img, Some(svg))
    }

    /// Whether the SVG this plugin makes is drawn in this process (and drawn again so): a trusted
    /// origin. Otherwise it is drawn by the supervised drawer every time.
    pub fn trusts_svg(&self) -> bool {
        self.trusted_origin
    }

    /// How the SVG this plugin makes is drawn, every time: in this process (`base` names it), or
    /// by the supervised drawer that reads no file.
    pub fn svg_trust(&self, base: &str) -> SvgTrust {
        if self.trusted_origin {
            SvgTrust::Trusted(PathBuf::from(base))
        } else {
            SvgTrust::Embedded
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    /// Test-only: a plugin that stands in for whatever the registry would name for an embedded
    /// kind, on this thread (the tests with no worker channel run the render on the test thread).
    pub static EMBED_STAND_IN: std::cell::RefCell<Option<Arc<dyn Plugin>>> =
        const { std::cell::RefCell::new(None) };
}

/// The [`EmbedRender`] of the plugin `registry` names for the embedded `kind`; `None` when no
/// enabled plugin takes it. The host says how a plugin id becomes its settings (`settings_of`) and
/// whether its origin is trusted to draw its own SVG (`trusted_origin_of`: decided by where the
/// plugin comes from, never by what it claims), and gives the drawer for the SVGs that are not
/// drawn in this process.
pub fn embed_render_for(
    registry: &Registry,
    kind: &str,
    lang: &str,
    settings_of: &dyn Fn(&str) -> Settings,
    trusted_origin_of: &dyn Fn(&str) -> bool,
    svg: Arc<dyn SvgDraw + Send + Sync>,
) -> Option<EmbedRender> {
    let id = registry.resolve_embed(kind)?;
    let plugin = registry.plugin(&id)?;
    #[cfg(any(test, feature = "test-hooks"))]
    let plugin = EMBED_STAND_IN
        .with(|s| s.borrow().clone())
        .unwrap_or(plugin);
    Some(EmbedRender::new(
        plugin,
        settings_of(&id),
        lang,
        trusted_origin_of(&id),
        svg,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use image::{DynamicImage, GenericImageView, RgbaImage};

    use super::*;
    use crate::registry::PluginOrigin;
    use crate::{Failure, Manifest, Session};

    const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#ff0000"/></svg>"##;

    /// What the plugin answers to the view request.
    #[derive(Clone, Copy)]
    enum Answer {
        Svg { trusted: bool },
        Fails,
        Text,
    }

    struct Fake {
        answer: Answer,
        refuse_open: bool,
    }

    struct FakeSession(Answer);

    impl Plugin for Fake {
        fn manifest(&self) -> Manifest {
            Manifest::new("fake", "0").bind(crate::manifest::Bind::embed("math"))
        }
        fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            if self.refuse_open {
                return Err(Failure::unsupported(crate::Tr::new("no")));
            }
            Ok(Box::new(FakeSession(self.answer)))
        }
    }

    impl Session for FakeSession {
        fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
            match self.0 {
                Answer::Svg { trusted } => Ok(View::Image(ImageView::new(ImageData::Svg {
                    data: Arc::from(RED_SVG.as_bytes()),
                    base_dir: None,
                    trusted,
                }))),
                Answer::Fails => Err(Failure::unsupported(crate::Tr::new("no"))),
                Answer::Text => Ok(View::text(["words"])),
            }
        }
    }

    /// A drawer that counts its calls and returns a 3 x 2 picture; the process-wide counters are
    /// per instance, so parallel tests do not mix.
    #[derive(Default)]
    struct Counting {
        untrusted: AtomicU32,
        embedded: AtomicU32,
        fail: bool,
    }

    impl SvgDraw for Arc<Counting> {
        fn untrusted(
            &self,
            _: &[u8],
            _: &Path,
            _: Option<&[std::path::PathBuf]>,
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            self.untrusted.fetch_add(1, Ordering::SeqCst);
            Ok(DynamicImage::ImageRgba8(RgbaImage::new(3, 2)))
        }
        fn embedded(
            &self,
            _: &[u8],
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            self.embedded.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(konoma_svg::svg::RasterError::Cancelled);
            }
            Ok(DynamicImage::ImageRgba8(RgbaImage::new(3, 2)))
        }
    }

    fn render(
        answer: Answer,
        trusted_origin: bool,
        fail_draw: bool,
    ) -> (EmbedRender, Arc<Counting>) {
        let drawer = Arc::new(Counting {
            fail: fail_draw,
            ..Counting::default()
        });
        let r = EmbedRender::new(
            Arc::new(Fake {
                answer,
                refuse_open: false,
            }),
            Settings::default(),
            "en",
            trusted_origin,
            Arc::new(drawer.clone()),
        );
        (r, drawer)
    }

    fn calls(d: &Counting) -> (u32, u32) {
        (
            d.untrusted.load(Ordering::SeqCst),
            d.embedded.load(Ordering::SeqCst),
        )
    }

    #[test]
    fn a_trusted_origin_is_drawn_in_this_process_and_kept() {
        let (r, drawer) = render(Answer::Svg { trusted: true }, true, false);
        let (img, svg) = r.picture("math", "x", false, 100, "math", "math.svg");
        let img = img.expect("a raster");
        assert_eq!(img.dimensions(), (100, 50), "the SVG's own size at max_px");
        let (w, h) = img.dimensions();
        assert_eq!(&img.get_pixel(w / 2, h / 2).0[..3], &[255, 0, 0]);
        assert_eq!(svg.expect("the svg is kept").as_slice(), RED_SVG.as_bytes());
        assert_eq!(calls(&drawer), (0, 0), "the supervised drawer was not used");
        assert!(r.trusts_svg());
        assert_eq!(
            r.svg_trust("math.svg"),
            SvgTrust::Trusted("math.svg".into())
        );
    }

    #[test]
    fn any_other_origin_is_drawn_by_the_supervised_drawer_that_reads_no_file() {
        let (r, drawer) = render(Answer::Svg { trusted: false }, false, false);
        let (img, svg) = r.picture("math", "x", false, 100, "math", "math.svg");
        assert_eq!(
            img.expect("a raster").dimensions(),
            (3, 2),
            "the drawer's picture"
        );
        assert!(svg.is_some());
        assert_eq!(calls(&drawer), (0, 1), "embedded, not a file");
        assert!(!r.trusts_svg());
        assert_eq!(r.svg_trust("math.svg"), SvgTrust::Embedded);
    }

    #[test]
    fn a_plugin_that_claims_trusted_from_an_untrusted_origin_is_still_drawn_by_the_drawer() {
        let (r, drawer) = render(Answer::Svg { trusted: true }, false, false);
        let (img, _) = r.picture("math", "x", false, 100, "math", "math.svg");
        assert_eq!(img.expect("a raster").dimensions(), (3, 2));
        assert_eq!(calls(&drawer), (0, 1));
    }

    #[test]
    fn a_trusted_origin_that_does_not_vouch_for_its_svg_is_refused() {
        let (r, drawer) = render(Answer::Svg { trusted: false }, true, false);
        let (img, svg) = r.picture("math", "x", false, 100, "mermaid", "mermaid.svg");
        assert_eq!(img.unwrap_err(), "mermaid render failed");
        assert!(svg.is_none());
        assert_eq!(calls(&drawer), (0, 0));
    }

    #[test]
    fn what_the_plugin_cannot_answer_is_a_failure_text_with_no_svg() {
        for answer in [Answer::Fails, Answer::Text] {
            let (r, _) = render(answer, true, false);
            let (img, svg) = r.picture("math", "x", false, 100, "formula", "math.svg");
            assert_eq!(img.unwrap_err(), "formula render failed");
            assert!(svg.is_none());
        }
        let r = EmbedRender::new(
            Arc::new(Fake {
                answer: Answer::Svg { trusted: true },
                refuse_open: true,
            }),
            Settings::default(),
            "en",
            true,
            Arc::new(Arc::new(Counting::default())),
        );
        let (img, svg) = r.picture("math", "x", false, 100, "formula", "math.svg");
        assert_eq!(img.unwrap_err(), "formula render failed");
        assert!(svg.is_none());
    }

    #[test]
    fn a_drawer_that_fails_gives_the_rasterize_text_but_keeps_the_svg() {
        let (r, drawer) = render(Answer::Svg { trusted: false }, false, true);
        let (img, svg) = r.picture("math", "x", false, 100, "math", "math.svg");
        assert_eq!(img.unwrap_err(), "rasterize failed");
        assert!(svg.is_some());
        assert_eq!(calls(&drawer), (0, 1));
    }

    fn registry_with(origin: PluginOrigin) -> Registry {
        let mut reg = Registry::new();
        reg.register(
            Arc::new(Fake {
                answer: Answer::Svg { trusted: true },
                refuse_open: false,
            }),
            origin,
        )
        .expect("registered");
        reg
    }

    #[test]
    fn the_registry_names_the_plugin_and_the_host_decides_the_trust() {
        let reg = registry_with(PluginOrigin::User);
        let svg: Arc<dyn SvgDraw + Send + Sync> = Arc::new(Arc::new(Counting::default()));
        let seen = std::cell::RefCell::new(Vec::new());
        let r = embed_render_for(
            &reg,
            "math",
            "ja",
            &|id| {
                seen.borrow_mut().push(id.to_string());
                Settings::default()
            },
            // The plugin claims `trusted`; the host's answer by id is what counts.
            &|_| false,
            svg.clone(),
        )
        .expect("the plugin takes math");
        assert_eq!(seen.borrow().as_slice(), ["fake"]);
        assert_eq!(r.lang(), "ja");
        assert!(!r.trusts_svg());
        let r = embed_render_for(
            &reg,
            "math",
            "ja",
            &|_| Settings::default(),
            &|_| true,
            svg.clone(),
        )
        .expect("the plugin takes math");
        assert!(r.trusts_svg());
        // A kind nobody takes.
        assert!(embed_render_for(
            &reg,
            "fence:other",
            "en",
            &|_| Settings::default(),
            &|_| true,
            svg
        )
        .is_none());
    }

    #[test]
    fn the_stand_in_replaces_the_plugin_the_registry_names() {
        let reg = registry_with(PluginOrigin::Standard);
        let svg: Arc<dyn SvgDraw + Send + Sync> = Arc::new(Arc::new(Counting::default()));
        EMBED_STAND_IN.with(|s| {
            *s.borrow_mut() = Some(Arc::new(Fake {
                answer: Answer::Fails,
                refuse_open: false,
            }))
        });
        let r = embed_render_for(&reg, "math", "en", &|_| Settings::default(), &|_| true, svg)
            .expect("taken");
        EMBED_STAND_IN.with(|s| *s.borrow_mut() = None);
        let (img, _) = r.picture("math", "x", false, 100, "math", "math.svg");
        assert_eq!(
            img.unwrap_err(),
            "math render failed",
            "the stand-in answered"
        );
    }
}
