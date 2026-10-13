//! konoma's `mermaid` plugin: a diagram drawn as a picture, or as Unicode text.
//!
//! It takes two kinds of input:
//!
//! - a file (`*.mmd`, `*.mermaid`), the whole file being one diagram;
//! - the embedded kind `fence:mermaid` (`Input::Embed`): the body of a ```` ```mermaid ```` fence
//!   of a Markdown document, handed over by the host.
//!
//! The view it returns depends on the **representation** the host asks for (`ViewReq::repr`) and,
//! when none is named, on the `mode` setting:
//!
//! - `"image"` (the default): [`View::Image`] holding the SVG that `konoma-mermaid-core` draws,
//!   marked **trusted** (konoma's own renderer made it; the host rasterizes it in-process) and
//!   `sharp_zoom` (the host redraws it sharply when the user zooms). A diagram that cannot be
//!   drawn (unsupported kind, parse failure) is a [`Failure`] of kind `Unsupported`; the host then
//!   asks again for the text representation, which is what the user sees instead.
//! - `"text"`: [`View::Text`] with the Unicode box-drawing diagram (`mermaid-text`) fitted to
//!   `ViewReq::cols`, or the dimmed raw source with a note when that fails. This is the same
//!   output the Markdown renderer gives a fence it does not draw as a picture.
//!
//! Settings (`[plugins.mermaid]`, or the old `[ui] mermaid*` names the host fills in):
//! `mode` (`image`/`text`), `theme`, `curve`, `routing`, and `rows` (read by the host: the
//! largest height of an inline diagram, in rows).

use std::path::PathBuf;
use std::sync::Arc;

use konoma_plugin::style::{Line, Modifier, Span, Style};
use konoma_plugin::view::{ImageData, ImageView, Layout, ReprDecl, TextSource, TextView};
use konoma_plugin::{
    t, Bind, Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, Session, SettingDecl,
    Unavailable, Value, Verb, VerbDecl, View, ViewCx, ViewKind, ViewReq,
};

/// The id of the plugin.
pub const MERMAID_ID: &str = "mermaid";

/// The embedded kind of a ```` ```mermaid ```` fence.
pub const FENCE_KIND: &str = "fence:mermaid";

/// The representation names a [`ViewReq`] may ask for.
pub const REPR_IMAGE: &str = "image";
pub const REPR_TEXT: &str = "text";

/// The defaults of the settings (the defaults of the old `[ui] mermaid*` items).
pub const DEFAULT_MODE: &str = "image";
pub const DEFAULT_THEME: &str = "dark";
pub const DEFAULT_CURVE: &str = "basis";
pub const DEFAULT_ROUTING: &str = "splines";
pub const DEFAULT_ROWS: i64 = 24;

/// The declarations of the `mermaid` plugin.
pub fn mermaid_manifest() -> Manifest {
    Manifest::new(MERMAID_ID, env!("CARGO_PKG_VERSION"))
        .name(t!(en: "Mermaid", ja: "Mermaid"))
        .bind(Bind::files(["*.{mmd,mermaid}"]))
        .bind(Bind::embed(FENCE_KIND))
        .shows(ViewKind::Image)
        .unavailable(Unavailable::RawText { with_reason: false })
        .setting(SettingDecl::new(
            "mode",
            Value::Str(DEFAULT_MODE.into()),
            t!(en: "image draws a picture, text draws box characters", ja: "image は絵、text は罫線文字で描く"),
        ))
        .setting(SettingDecl::new(
            "theme",
            Value::Str(DEFAULT_THEME.into()),
            t!(en: "Color theme of the diagram", ja: "図の配色"),
        ))
        .setting(SettingDecl::new(
            "curve",
            Value::Str(DEFAULT_CURVE.into()),
            t!(en: "Line shape of the edges", ja: "線の形"),
        ))
        .setting(SettingDecl::new(
            "routing",
            Value::Str(DEFAULT_ROUTING.into()),
            t!(en: "Edge routing: splines or konoma-orthogonal", ja: "配線: splines か konoma-orthogonal"),
        ))
        .setting(SettingDecl::new(
            "rows",
            Value::Int(DEFAULT_ROWS),
            t!(en: "Largest height of an inline diagram, in rows", ja: "インライン図の最大の高さ(行)"),
        ))
        // `edit` opens the user's editor in the terminal; `open` hands the file to the OS default
        // application as a GUI program.
        .verb(
            VerbDecl::new(Verb::Edit)
                .launcher(Launcher::Editor)
                .mode(LaunchMode::Terminal)
                .line_token(),
        )
        .verb(
            VerbDecl::new(Verb::Open)
                .launcher(Launcher::OsDefault)
                .mode(LaunchMode::Gui),
        )
}

/// Warm the renderer's lazy font measurement (it hides the first-render stall). Safe to call from
/// any thread, any number of times.
pub fn warm() {
    konoma_mermaid_core::warm_mermaid();
}

/// The Unicode text diagram of `code`, fitted to `width` columns (`0` = no limit). Failure,
/// an unsupported diagram or an internal panic of the text renderer gives the dimmed raw source
/// under a one-line note, so the user always sees something.
pub fn text_diagram(code: &str, width: u16) -> Vec<Line> {
    let max_width = (width != 0).then_some(width as usize);
    // `mermaid-text` can panic rather than return an error on some inputs; the panic hook is
    // silenced for the call so the message does not land on the screen.
    let caught = konoma_base::panic::silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mermaid_text::render_with_width(code.trim_end_matches('\n'), max_width)
        }))
    });
    let note = match caught {
        Ok(Ok(rendered)) => return rendered.lines().map(line_of).collect(),
        Ok(Err(e)) => format!("cannot render mermaid: {e}"),
        Err(_) => "cannot render mermaid (internal error: this diagram/char may be unsupported)"
            .to_string(),
    };
    raw_with_note(code, &note)
}

/// One line of text. An empty one has no span at all (what the Markdown renderer's own text
/// diagram gives), so the host draws exactly the lines it always drew.
fn line_of(text: &str) -> Line {
    if text.is_empty() {
        Line::from(Vec::<Span>::new())
    } else {
        Line::raw(text)
    }
}

/// The dimmed raw source under a note (what the user sees when a diagram cannot be drawn).
fn raw_with_note(code: &str, note: &str) -> Vec<Line> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut v = vec![Line::from(Span::styled(format!("[{note}]"), dim))];
    for l in code.lines() {
        v.push(Line::from(Span::styled(format!("  {l}"), dim)));
    }
    v
}

/// The mermaid plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct MermaidPlugin;

impl Plugin for MermaidPlugin {
    fn manifest(&self) -> Manifest {
        mermaid_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let source = match input {
            Input::File { path, .. } => Source::File(path),
            Input::Embed { kind, source, .. } if kind == FENCE_KIND => Source::Text(source),
            _ => {
                return Err(Failure::unsupported(t!(
                    en: "the mermaid plugin opens a file or a mermaid fence only",
                    ja: "mermaid プラグインが開けるのはファイルか mermaid フェンスだけです",
                )))
            }
        };
        Ok(Box::new(MermaidSession { source }))
    }
}

/// The most bytes of a diagram file that are read (the same as a Markdown file: past it the
/// preview does not need the rest, and a multi-gigabyte file named `.mmd` must not be read whole).
pub const MAX_BYTES: u64 = 1024 * 1024;

/// Read a diagram file: a regular file of at most [`MAX_BYTES`] bytes of UTF-8 (as
/// `read_to_string` was). Anything else is refused with the reason, never read into memory.
fn read_limited(path: &std::path::Path) -> Result<String, Failure> {
    use std::io::Read as _;
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(Failure::unsupported(t!(
            en: "this is not a regular file",
            ja: "通常のファイルではありません",
        )));
    }
    if meta.len() > MAX_BYTES {
        return Err(Failure::too_large(t!(
            en: "[mermaid] not drawn: the file is too large",
            ja: "[mermaid] 描画しません: ファイルが大きすぎます",
        )));
    }
    // The size was read before the file: a file that grows meanwhile is still cut at the limit.
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_BYTES {
        return Err(Failure::too_large(t!(
            en: "[mermaid] not drawn: the file is too large",
            ja: "[mermaid] 描画しません: ファイルが大きすぎます",
        )));
    }
    String::from_utf8(buf).map_err(|e| {
        Failure::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.utf8_error().to_string(),
        ))
    })
}

enum Source {
    /// A file, read each time a view is made (so an edit shows).
    File(PathBuf),
    /// The body of a fence, or text the host already read.
    Text(String),
}

struct MermaidSession {
    source: Source,
}

impl MermaidSession {
    fn code(&self) -> Result<String, Failure> {
        match &self.source {
            Source::File(p) => read_limited(p),
            Source::Text(s) => Ok(s.clone()),
        }
    }
}

impl Session for MermaidSession {
    fn layout(&self) -> Layout {
        Layout {
            reprs: vec![
                ReprDecl::new(REPR_IMAGE, t!(en: "Picture", ja: "絵")),
                ReprDecl::new(REPR_TEXT, t!(en: "Text", ja: "文字")),
            ],
            default_repr: Some(REPR_IMAGE.into()),
            default_repr_without_pixels: None,
            pages: None,
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let want_text = match req.repr.as_deref() {
            Some(REPR_TEXT) => true,
            Some(_) => false,
            None => cx.settings.str("mode", DEFAULT_MODE) == "text",
        };
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let code = self.code()?;
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        if want_text {
            return Ok(View::Text(TextView {
                source: TextSource::Lines(text_diagram(&code, req.cols)),
                styler: None,
                file_lines: false,
                syntax_name: None,
            }));
        }
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let svg = konoma_mermaid_core::mermaid_to_svg_flow(
            &code,
            cx.settings.str("theme", DEFAULT_THEME),
            cx.settings.str("curve", DEFAULT_CURVE),
            cx.settings.str("routing", DEFAULT_ROUTING),
        )
        .ok_or_else(|| {
            Failure::unsupported(t!(
                en: "this diagram cannot be drawn as a picture",
                ja: "この図は絵として描けません",
            ))
        })?;
        Ok(View::Image(ImageView {
            data: ImageData::Svg {
                data: Arc::from(svg.into_bytes()),
                base_dir: None,
                // konoma's own renderer made it: the host rasterizes it in-process.
                trusted: true,
            },
            sharp_zoom: true,
            raster_max_px: None,
        }))
    }
}

#[cfg(test)]
mod tests;
