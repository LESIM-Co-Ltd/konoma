//! Embedded kinds on the host side: a ```` ```mermaid ```` fence and a formula in a Markdown
//! document are drawn by the plugin the registry names for the kind (`fence:mermaid`, `math`), not
//! by a function the host calls directly.
//!
//! The flow, for one fence or formula:
//!
//! 1. The document's render asks for the picture (`ensure_mermaid_fence_render`,
//!    `ensure_math_render`, the full-screen fence job). Which plugin takes the kind is settled on
//!    the UI thread by [`App::embed_render`] (`Registry::resolve_embed`; `None` when the plugin is
//!    switched off, and the fence / formula is then shown the way it is when it cannot be drawn).
//! 2. The worker (or the caller, in tests with no channel) opens the plugin with
//!    `Input::Embed { kind, source, display }` and asks for its view ([`EmbedRender::picture`]).
//! 3. The view is a picture whose SVG the host rasterizes: in-process for a standard plugin that
//!    draws its own SVG and vouches for it (`trusted`; mermaid and math), in the supervised child
//!    process (no file is read) for any other plugin, whatever it claims.
//! 4. The raster and the SVG go to the same place they always went: the `md_image_cache` entry under
//!    the synthetic key (`mermaid-fence://`, `math://`), which the document's layout and the kitty
//!    pipeline read. Everything about those keys (who may rebuild them, when they are swept, the
//!    in-flight cap, the request ids) is unchanged and lives in `md_media.rs`.
//!
//! What the plugin owns is only "source in, SVG out". How big a formula is in the text, and where
//! an inline one sits on its line, is the document's layout.

use konoma_plugin::view::{ImageData, ImageView, TextSource};
use konoma_plugin::{Input, OpenCx, Plugin, Settings, View, ViewCx, ViewReq};

use super::*;

/// The embedded kinds the host asks plugins for.
pub(super) const FENCE_MERMAID: &str = "fence:mermaid";
pub(super) const MATH: &str = "math";

/// The representation name of the text diagram (the mermaid plugin's `Layout` names it).
pub(super) const REPR_TEXT: &str = konoma_mermaid::REPR_TEXT;

/// What a worker needs to ask an embed plugin for a picture (owned: it moves to a thread).
#[derive(Clone)]
pub(super) struct EmbedRender {
    plugin: Arc<dyn Plugin>,
    settings: Settings,
    lang: String,
    /// Whether the host believes this plugin's `trusted` claim: it is a standard plugin that draws
    /// its own SVG (`Profile::own_svg`). Decided by where the plugin comes from, never by
    /// what it says.
    trusted_origin: bool,
}

/// The raster and the SVG it was made from, or the reason there is none.
pub(super) type EmbedPicture = (Result<image::DynamicImage, String>, Option<Arc<Vec<u8>>>);

impl EmbedRender {
    /// Open the plugin on `source` and rasterize the SVG its view holds at `max_px` (the longest
    /// side). `label` names the thing for the failure text; `base` is the file name the rasterizer
    /// resolves relative references from (the SVGs konoma draws have none).
    ///
    /// An SVG from a plugin whose origin the host trusts is drawn in this process, and only when
    /// the plugin vouches for it (`trusted`): the host keeps the SVG and rasterizes it again later
    /// (sharp zoom, rebuilding an evicted raster) in this process too. An SVG from any other
    /// plugin is drawn by the supervised child process, with no file to read, and is drawn the
    /// same way every time it is made again (see [`EmbedRender::trusts_svg`]).
    pub(super) fn picture(
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
        // The retained copy is what the md entry keeps for the layout and for later rasters.
        let svg = Arc::new(data.to_vec());
        let img = if self.trusted_origin {
            // A plugin we trust that does not vouch for its own SVG is a bug in it: refuse, so an
            // entry is never half trusted.
            if !trusted {
                return failed();
            }
            crate::preview::svg::rasterize_trusted(&svg, Path::new(base), max_px)
                .ok_or_else(|| "rasterize failed".to_string())
        } else {
            crate::preview::svg::rasterize_embedded(&svg, max_px, &|| false)
                .map_err(|_| "rasterize failed".to_string())
        };
        (img, Some(svg))
    }

    /// Whether the SVG this plugin makes is drawn in this process (and drawn again so): a trusted
    /// origin. Otherwise it is drawn in the child process every time.
    pub(super) fn trusts_svg(&self) -> bool {
        self.trusted_origin
    }
}

/// Test seam: whether the host believes the `fence:mermaid` plugin of `cfg`, and what drawing a
/// fence with it gives (the picture, and whether the SVG was kept).
#[cfg(test)]
pub(crate) fn probe_fence_picture(
    cfg: crate::config::Config,
) -> (bool, Result<image::DynamicImage, String>, bool) {
    let dir = std::env::temp_dir();
    let app = App::new(dir, cfg).expect("an app");
    let embed = app
        .embed_render(FENCE_MERMAID)
        .expect("a plugin takes the fence");
    let (img, svg) = embed.picture(
        FENCE_MERMAID,
        "graph TD;A-->B",
        false,
        100,
        "mermaid",
        "mermaid.svg",
    );
    (embed.trusts_svg(), img, svg.is_some())
}

#[cfg(test)]
thread_local! {
    /// Test-only: a plugin that stands in for whatever the registry would name for an embedded
    /// kind, on this thread (the tests with no worker channel run the render on the test thread).
    pub(super) static EMBED_STAND_IN: std::cell::RefCell<Option<Arc<dyn Plugin>>> =
        const { std::cell::RefCell::new(None) };
}

impl App {
    /// The plugin that takes the embedded `kind`, with the settings and language it is opened
    /// with. `None` when no enabled plugin takes it (switched off with
    /// `[plugins.<id>] enabled = false`).
    pub(super) fn embed_render(&self, kind: &str) -> Option<EmbedRender> {
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let id = registry.resolve_embed(kind)?;
        let plugin = registry.plugin(&id)?;
        #[cfg(test)]
        let plugin = EMBED_STAND_IN
            .with(|s| s.borrow().clone())
            .unwrap_or(plugin);
        Some(EmbedRender {
            plugin,
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            lang: self.plugin_lang().to_string(),
            trusted_origin: crate::plugins::profile_of(&id).own_svg,
        })
    }

    /// Whether an enabled plugin takes the embedded `kind` (cheap: no settings are built).
    pub(super) fn embed_plugin_enabled(&self, kind: &str) -> bool {
        crate::plugins::shared_registry(&self.cfg.plugin_disabled())
            .resolve_embed(kind)
            .is_some()
    }
}

impl App {
    /// The text diagram of a standalone mermaid file that is shown as text (`mode = "text"`, no
    /// image backend, or a diagram the picture could not be made of). `src` is the file's text as
    /// the host read it (with its display limits); `width` is the display width in columns. The
    /// lines are the mermaid plugin's own text representation, so they are what the Markdown
    /// renderer draws for a fence it does not draw as a picture.
    ///
    /// It runs on the UI thread, inside the decorated render, as the text diagram always did: it is
    /// a pure function of the text and the width (a resize asks again at the new width), and the
    /// plugin work is a short string layout, not a picture.
    pub(super) fn mermaid_text_lines(&self, src: &str, width: u16) -> Vec<Line<'static>> {
        let Some(embed) = self.embed_render(FENCE_MERMAID) else {
            return unavailable_lines(src, None);
        };
        let input = Input::Embed {
            kind: FENCE_MERMAID.to_string(),
            source: src.to_string(),
            display: false,
        };
        let open_cx = OpenCx::new(&embed.settings, &embed.lang);
        let mut session = match embed.plugin.open(input, &open_cx) {
            Ok(s) => s,
            Err(f) => return unavailable_lines(src, Some(&f)),
        };
        let req = ViewReq {
            repr: Some(REPR_TEXT.to_string()),
            cols: width,
            rows: self.tab.preview_viewport.max(1),
            ..ViewReq::default()
        };
        let view_cx = ViewCx::new(&embed.settings, &embed.lang);
        match session.view(&req, &view_cx) {
            Ok(View::Text(t)) => match t.source {
                TextSource::Lines(lines) => lines
                    .into_iter()
                    .map(super::text_plugin::ratatui_line)
                    .collect(),
                TextSource::Owned(text) => {
                    text.lines().map(|l| Line::from(l.to_string())).collect()
                }
                TextSource::File(_) => unavailable_lines(src, None),
            },
            Ok(_) => unavailable_lines(src, None),
            Err(f) => unavailable_lines(src, Some(&f)),
        }
    }
}

/// The dimmed raw source under a one-line note: what is left when the plugin cannot give the text
/// diagram at all.
fn unavailable_lines(src: &str, why: Option<&konoma_plugin::Failure>) -> Vec<Line<'static>> {
    use ratatui::style::Stylize;
    let note = match why {
        Some(f) => format!("cannot render mermaid: {f}"),
        None => "cannot render mermaid: the mermaid plugin is not available".to_string(),
    };
    let mut v = vec![Line::from(Span::from(format!("[{note}]")).dim())];
    for l in src.lines() {
        v.push(Line::from(Span::from(format!("  {l}")).dim()));
    }
    v
}
