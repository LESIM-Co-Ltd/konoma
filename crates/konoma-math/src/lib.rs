//! konoma's `math` plugin: a LaTeX formula drawn as a picture.
//!
//! It takes the embedded kinds `math` (a formula inside Markdown: inline `$..$` or a block `$$..$$`,
//! told apart by `display`) and `fence:math`. The host hands the formula's source over with
//! `Input::Embed` and gets back [`View::Image`] holding the SVG that `konoma-math-core` draws
//! (RaTeX, pure Rust), marked **trusted** because konoma's own code made it. The formula's size in
//! the text (how many cells it takes, an inline formula sitting on a text line) is the host's
//! document layout; the plugin only draws.
//!
//! Settings (`[plugins.math]`, or the old `[ui] math*` names the host fills in):
//! - `mode` (`image` by default): `text` keeps the source as text (the host does not even ask
//!   for a picture then);
//! - `color` (`#d0d0d0` by default): the color of the glyphs. RaTeX paints pure black, which is
//!   invisible on a dark terminal. The host hands over a value it has already checked against
//!   the parser the rasterizer uses.

use std::sync::Arc;

use konoma_plugin::style::Line;
use konoma_plugin::view::{ImageData, ImageView, TextSource, TextView};
use konoma_plugin::{
    t, Bind, Failure, Input, Manifest, OpenCx, Plugin, Session, SettingDecl, Value, View, ViewCx,
    ViewReq,
};

/// The id of the plugin.
pub const MATH_ID: &str = "math";

/// The embedded kinds the plugin takes.
pub const MATH_KIND: &str = "math";
pub const FENCE_KIND: &str = "fence:math";

pub const DEFAULT_MODE: &str = "image";
pub const DEFAULT_COLOR: &str = "#d0d0d0";

/// The declarations of the `math` plugin. It takes formulas inside Markdown only, so it has no
/// file to edit or open (no launch verbs).
pub fn math_manifest() -> Manifest {
    Manifest::new(MATH_ID, env!("CARGO_PKG_VERSION"))
        .name(t!(en: "Math", ja: "数式"))
        .bind(Bind::embed(FENCE_KIND))
        .bind(Bind::embed(MATH_KIND))
        .setting(SettingDecl::new(
            "mode",
            Value::Str(DEFAULT_MODE.into()),
            t!(en: "image draws a picture, text keeps the source", ja: "image は絵、text はソースのまま"),
        ))
        .setting(SettingDecl::new(
            "color",
            Value::Str(DEFAULT_COLOR.into()),
            t!(en: "Color of the formulas", ja: "数式の色"),
        ))
}

/// The math plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct MathPlugin;

impl Plugin for MathPlugin {
    fn manifest(&self) -> Manifest {
        math_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        match input {
            Input::Embed {
                kind,
                source,
                display,
            } if kind == MATH_KIND || kind == FENCE_KIND => Ok(Box::new(MathSession {
                latex: source,
                // A ```math fence is a block formula.
                display: display || kind == FENCE_KIND,
            })),
            _ => Err(Failure::unsupported(t!(
                en: "the math plugin opens a formula inside Markdown only",
                ja: "math プラグインが開けるのは Markdown 内の数式だけです",
            ))),
        }
    }
}

struct MathSession {
    latex: String,
    display: bool,
}

impl Session for MathSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        if cx.settings.str("mode", DEFAULT_MODE) == "text" {
            return Ok(View::Text(TextView {
                source: TextSource::Lines(self.latex.lines().map(Line::raw).collect()),
                styler: None,
                file_lines: false,
                syntax_name: None,
            }));
        }
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let svg = konoma_math_core::latex_to_svg(
            &self.latex,
            self.display,
            cx.settings.str("color", DEFAULT_COLOR),
        )
        .ok_or_else(|| {
            Failure::unsupported(t!(
                en: "this formula cannot be drawn",
                ja: "この数式は描けません",
            ))
        })?;
        Ok(View::Image(ImageView {
            data: ImageData::Svg {
                data: Arc::from(svg.into_bytes()),
                base_dir: None,
                trusted: true,
            },
            sharp_zoom: false,
            raster_max_px: None,
        }))
    }
}

#[cfg(test)]
mod tests;
