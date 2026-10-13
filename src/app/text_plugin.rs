//! The `code` plugin on the host side: the text display type.
//!
//! The plugin's `view` is a [`TextView`] whose [`LineStyler`] colors lines. Everything else about
//! text stays the host's, in `windowed_lines`: reading the file in windows (`FileWindow`: the
//! source of the text is the file itself), tab expansion, the search highlight, the caret and
//! the `v`/`V` selection, line numbers, the git gutter, wrapping and horizontal scroll, and the
//! line numbers `e` and `Y` use. The host asks the styler for the visible window only, so a huge
//! file is never colored whole.
//!
//! The text the plugin does not own (a plain-text file whose name still gives a grammar, the raw
//! source of Markdown and of a Word document) is colored by the very same styler, built here.

use ratatui::style::{Modifier, Style};

use konoma_plugin::view::{LineStyler, TextView, View};
use konoma_plugin::{Color as PColor, Line as PLine, Modifier as PModifier, Style as PStyle};

use super::*;

fn color_of(c: PColor) -> ratatui::style::Color {
    use ratatui::style::Color as R;
    match c {
        PColor::Reset => R::Reset,
        PColor::Black => R::Black,
        PColor::Red => R::Red,
        PColor::Green => R::Green,
        PColor::Yellow => R::Yellow,
        PColor::Blue => R::Blue,
        PColor::Magenta => R::Magenta,
        PColor::Cyan => R::Cyan,
        PColor::Gray => R::Gray,
        PColor::DarkGray => R::DarkGray,
        PColor::LightRed => R::LightRed,
        PColor::LightGreen => R::LightGreen,
        PColor::LightYellow => R::LightYellow,
        PColor::LightBlue => R::LightBlue,
        PColor::LightMagenta => R::LightMagenta,
        PColor::LightCyan => R::LightCyan,
        PColor::White => R::White,
        PColor::Indexed(i) => R::Indexed(i),
        PColor::Rgb(r, g, b) => R::Rgb(r, g, b),
    }
}

fn modifier_of(m: PModifier) -> Modifier {
    // The bits are at the same positions (the API promises it); bits ratatui does not know are
    // dropped.
    Modifier::from_bits_truncate(m.bits())
}

fn style_of(s: PStyle) -> Style {
    let mut out = Style::default()
        .add_modifier(modifier_of(s.add))
        .remove_modifier(modifier_of(s.sub));
    out.fg = s.fg.map(color_of);
    out.bg = s.bg.map(color_of);
    out
}

/// A plugin's line as the host draws it.
pub(super) fn ratatui_line(l: PLine) -> Line<'static> {
    let spans: Vec<Span<'static>> = l
        .spans
        .into_iter()
        .map(|s| Span::styled(s.text, style_of(s.style)))
        .collect();
    Line::from(spans).style(style_of(l.style))
}

impl App {
    /// Whether the preview is a file the `code` plugin shows.
    pub fn shows_code_plugin(&self) -> bool {
        self.tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_code_plugin)
    }

    /// The styler of the open `code` show, once its first result has landed (and only when the
    /// plugin chose to color: it returns none with `syntax_highlight` off).
    fn code_styler(&mut self) -> Option<&mut Box<dyn LineStyler>> {
        if !self.shows_code_plugin() {
            return None;
        }
        let show = self.plugin.as_mut()?;
        if show.id != konoma_code::CODE_ID {
            return None;
        }
        match show.view.as_mut()? {
            View::Text(TextView { styler, .. }) => styler.as_mut(),
            _ => None,
        }
    }

    /// Whether the `code` plugin has not answered yet (its first result has not landed). The
    /// window is plain and uncached meanwhile; once it has answered, or failed, the window is
    /// colored: by the plugin's styler, or (a failed plugin) by the same styler built here.
    pub(super) fn code_plugin_pending(&self) -> bool {
        self.shows_code_plugin()
            && self
                .plugin
                .as_ref()
                .is_some_and(|p| matches!(p.phase, plugin_host::PluginPhase::Loading))
    }

    /// Color the lines of a window (`src`: whole lines, newline separated, starting at the
    /// 0-based `first_line`). The `code` plugin's own styler does it for its file; for other text
    /// the same styler is built from `grammar_path` (the name that picks the grammar).
    pub(super) fn style_text_window(
        &mut self,
        src: &str,
        first_line: usize,
        grammar_path: &Path,
    ) -> Vec<Line<'static>> {
        // `code_styler` only answers while the open show is the `code` plugin's.
        let theme = self.cfg.ui.theme.code_theme.clone();
        let lines = match self.code_styler() {
            Some(styler) => styler.style_block(first_line, src),
            None => konoma_code::CodeStyler::new(grammar_path.to_path_buf(), theme)
                .style_block(first_line, src),
        };
        lines.into_iter().map(ratatui_line).collect()
    }

    /// A `code` result landed: if the grammar got warm while the plugin was opening, the loading
    /// state ends (the styler's `ready` is the plugin's word on it).
    pub(super) fn text_view_landed(&mut self) {
        if !self.hl_pending {
            return;
        }
        if self.code_styler().is_some_and(|s| s.ready()) {
            self.clear_highlight_pending();
        }
    }
}
