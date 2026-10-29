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

use konoma_plugin::view::TextSource;
use konoma_plugin::{Input, OpenCx, View, ViewCx, ViewReq};

use super::*;

/// The embedded kinds the host asks plugins for.
pub(super) const FENCE_MERMAID: &str = "fence:mermaid";
pub(super) const MATH: &str = "math";

/// The representation name of the text diagram (the mermaid plugin's `Layout` names it).
pub(super) const REPR_TEXT: &str = konoma_mermaid::REPR_TEXT;

/// What a worker needs to ask an embed plugin for a picture (owned: it moves to a thread). The
/// drawing is shared with the standalone app (`konoma_plugin::canvas_pictures::EmbedRender`); this
/// host draws the SVGs that are not its own in a supervised child process (`SvgChild`).
pub(super) use konoma_plugin::canvas_pictures::EmbedRender;

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
pub(super) use konoma_plugin::canvas_pictures::EMBED_STAND_IN;

#[cfg(test)]
thread_local! {
    /// Test-only: how many times [`App::embed_render`] ran on this thread.
    pub(super) static EMBED_RENDER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl App {
    /// The plugin that takes the embedded `kind`, with the settings and language it is opened
    /// with. `None` when no enabled plugin takes it (switched off with
    /// `[plugins.<id>] enabled = false`).
    pub(super) fn embed_render(&self, kind: &str) -> Option<EmbedRender> {
        #[cfg(test)]
        EMBED_RENDER_CALLS.with(|c| c.set(c.get() + 1));
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        konoma_plugin::canvas_pictures::embed_render_for(
            &registry,
            kind,
            self.plugin_lang(),
            &|id| crate::plugins::effective_settings(&self.cfg, id),
            &|id| crate::plugins::profile_of(id).own_svg,
            Arc::new(super::media_plugin::SvgChild),
        )
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
        let open_cx = OpenCx::new(embed.settings(), embed.lang());
        let mut session = match embed.plugin().open(input, &open_cx) {
            Ok(s) => s,
            Err(f) => return unavailable_lines(src, Some(&f)),
        };
        let req = ViewReq::new(width, self.tab.preview_viewport.max(1)).repr(REPR_TEXT);
        let view_cx = ViewCx::new(embed.settings(), embed.lang());
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
