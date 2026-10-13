//! konoma's `code` plugin: source and config files, shown as text.
//!
//! The plugin returns a [`konoma_plugin::view::TextView`] whose [`LineStyler`] colors the lines
//! the host asks about (the visible window of the file). The host owns everything else about
//! text: reading the file in windows, the caret, `v`/`V` selection, search, line numbers, the git
//! gutter, wrapping and horizontal scroll, `e` at the caret line and `Y`. The coloring itself
//! (syntect with the two-face syntax set, the grammar picked from the file name) is
//! `konoma-code-core`, which Markdown code blocks and the diff view share.
//!
//! Settings (`[plugins.code]`, or the old `[ui]` name the host fills in):
//! - `syntax_highlight` (bool, default true): color the lines. Off returns a view with no styler.
//! - `code_theme` (string, default `TwoDark`): the syntect theme. The host fills it from
//!   `[ui.theme] code_theme`, which Markdown code blocks use too.

use std::path::PathBuf;

use konoma_plugin::style::{Color, Line, Modifier, Span, Style};
use konoma_plugin::view::{LineStyler, TextSource, TextView, View, ViewReq};
use konoma_plugin::{
    t, Bind, Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, Session, SettingDecl,
    Value, Verb, VerbDecl, ViewCx,
};

/// The id of the plugin.
pub const CODE_ID: &str = "code";

/// The theme used when the host passes none.
const DEFAULT_THEME: &str = "TwoDark";

/// The files the plugin takes by default (the old default rule of `builtin = "code"`).
const CODE_GLOB: &str = "*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}";

/// The declarations of the `code` plugin.
pub fn code_manifest() -> Manifest {
    Manifest::new(CODE_ID, env!("CARGO_PKG_VERSION"))
        .name(t!(en: "Source code", ja: "ソースコード"))
        .bind(Bind::files([CODE_GLOB]))
        .setting(SettingDecl::new(
            "syntax_highlight",
            Value::Bool(true),
            t!(en: "Color code and config files", ja: "コードと設定ファイルに色を付ける"),
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

/// Colors lines the way a code file is colored: the grammar comes from the file's name (then its
/// first line), the colors from the theme. Each call starts a fresh highlight state at the first
/// line it is given, which is what the host wants for a window onto a large file.
pub struct CodeStyler {
    path: PathBuf,
    theme: String,
}

impl CodeStyler {
    /// A styler for lines of `path`, colored with the syntect theme `theme` (unknown or empty
    /// names give the default theme).
    pub fn new(path: impl Into<PathBuf>, theme: impl Into<String>) -> CodeStyler {
        CodeStyler {
            path: path.into(),
            theme: theme.into(),
        }
    }

    fn ext(&self) -> &str {
        self.path.extension().and_then(|e| e.to_str()).unwrap_or("")
    }
}

impl LineStyler for CodeStyler {
    fn style_block(&mut self, _first_line: usize, text: &str) -> Vec<Line> {
        konoma_code_core::highlight(text, &self.path, &self.theme)
            .iter()
            .map(line_of)
            .collect()
    }

    /// Whether the grammar of this file's extension is compiled. Until it is, the first
    /// highlight stalls for a moment, so the host shows a note (or plain text) and warms the
    /// grammar on a worker first.
    fn ready(&self) -> bool {
        konoma_code_core::is_ext_warm(self.ext())
    }
}

/// The code plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct CodePlugin;

impl Plugin for CodePlugin {
    fn manifest(&self) -> Manifest {
        code_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        match input {
            Input::File { path, .. } => Ok(Box::new(CodeSession { path })),
            _ => Err(Failure::unsupported(t!(
                en: "this plugin opens files only",
                ja: "このプラグインはファイルだけを開きます",
            ))),
        }
    }
}

/// One opened file. The file is read by the host (in windows); the session only knows its name.
struct CodeSession {
    path: PathBuf,
}

impl Session for CodeSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let styler: Option<Box<dyn LineStyler>> = if cx.settings.bool("syntax_highlight", true) {
            Some(Box::new(CodeStyler::new(
                self.path.clone(),
                cx.settings.str("code_theme", DEFAULT_THEME),
            )))
        } else {
            None
        };
        Ok(View::Text(TextView {
            source: TextSource::File(self.path.clone()),
            styler,
            // Line numbers are the file's own (`e`, `Y` and paste jumps use them).
            file_lines: true,
            syntax_name: None,
        }))
    }
}

/// A ratatui color as the plugin API names it.
fn color_of(c: ratatui::style::Color) -> Color {
    use ratatui::style::Color as R;
    match c {
        R::Reset => Color::Reset,
        R::Black => Color::Black,
        R::Red => Color::Red,
        R::Green => Color::Green,
        R::Yellow => Color::Yellow,
        R::Blue => Color::Blue,
        R::Magenta => Color::Magenta,
        R::Cyan => Color::Cyan,
        R::Gray => Color::Gray,
        R::DarkGray => Color::DarkGray,
        R::LightRed => Color::LightRed,
        R::LightGreen => Color::LightGreen,
        R::LightYellow => Color::LightYellow,
        R::LightBlue => Color::LightBlue,
        R::LightMagenta => Color::LightMagenta,
        R::LightCyan => Color::LightCyan,
        R::White => Color::White,
        R::Indexed(i) => Color::Indexed(i),
        R::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// ratatui's modifier bits as the plugin API names them (the bits are at the same positions).
fn modifier_of(m: ratatui::style::Modifier) -> Modifier {
    use ratatui::style::Modifier as R;
    [
        (R::BOLD, Modifier::BOLD),
        (R::DIM, Modifier::DIM),
        (R::ITALIC, Modifier::ITALIC),
        (R::UNDERLINED, Modifier::UNDERLINED),
        (R::SLOW_BLINK, Modifier::SLOW_BLINK),
        (R::RAPID_BLINK, Modifier::RAPID_BLINK),
        (R::REVERSED, Modifier::REVERSED),
        (R::HIDDEN, Modifier::HIDDEN),
        (R::CROSSED_OUT, Modifier::CROSSED_OUT),
    ]
    .into_iter()
    .filter(|(r, _)| m.contains(*r))
    .fold(Modifier::EMPTY, |acc, (_, p)| acc | p)
}

fn style_of(s: ratatui::style::Style) -> Style {
    Style {
        fg: s.fg.map(color_of),
        bg: s.bg.map(color_of),
        add: modifier_of(s.add_modifier),
        sub: modifier_of(s.sub_modifier),
    }
}

fn line_of(l: &ratatui::text::Line<'_>) -> Line {
    Line {
        spans: l
            .spans
            .iter()
            .map(|s| Span::styled(s.content.to_string(), style_of(s.style)))
            .collect(),
        style: style_of(l.style),
    }
}

#[cfg(test)]
mod tests;
