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
//!
//! **Any plugin that declares `shows(Text)` gets this**, not only `code`. A plugin whose text is
//! the file itself (`Manifest::file_text`, the `code` plugin) is windowed at once; one whose text
//! is made from the file (a hex dump: `TextSource::Lines` or `Owned`) is waited for ("Loading..."),
//! and when its view lands the text is written to a private temp file the reader windows, so the
//! caret, `v`/`V`, `/`, line numbers and `y` work on it exactly as on a file. Its colors are kept
//! with it (the lines it returned are what a window shows). A `TextSource::File` of a file other
//! than the one opened is read in place.

use konoma_plugin::view::{LineStyler, TextSource, TextView, View};
use konoma_plugin::Line as PLine;

use super::plugin_host::MadeText;
use super::*;

/// A plugin's line as the host draws it.
pub(super) fn ratatui_line(l: PLine) -> Line<'static> {
    konoma_plugin::ratatui_draw::into_line(l)
}

impl App {
    /// Whether the preview is a file a plugin shows as text (it declares `shows(Text)`: the `code`
    /// plugin, a hex dump...).
    pub fn shows_text_plugin(&self) -> bool {
        self.tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_text_plugin)
    }

    /// Whether the preview is a text plugin whose text is the file itself (`file_text`): read in
    /// windows at once, before the plugin has answered.
    pub fn shows_file_text_plugin(&self) -> bool {
        self.tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_file_text)
    }

    /// Whether the preview is a text plugin whose text is made (not the file itself).
    pub(super) fn text_plugin_is_generated(&self) -> bool {
        self.shows_text_plugin() && !self.shows_file_text_plugin()
    }

    /// The text view the open plugin show holds.
    fn text_plugin_view(&self) -> Option<&TextView> {
        match self.plugin.as_ref()?.view.as_ref()? {
            View::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Whether the open text view has a styler of its own.
    pub(super) fn text_plugin_has_styler(&self) -> bool {
        self.text_plugin_view().is_some_and(|t| t.styler.is_some())
    }

    /// Whether the line numbers of the open text view are the numbers in the original file (what
    /// `e` and `Y` need). A plugin that does not say gets `false`.
    pub(super) fn text_plugin_lines_are_file_lines(&self) -> bool {
        self.text_plugin_view().is_some_and(|t| t.file_lines)
    }

    /// The file a made text plugin names as its text (`TextSource::File`), read in place.
    pub(super) fn text_plugin_own_file(&self) -> Option<&Path> {
        match &self.text_plugin_view()?.source {
            TextSource::File(p) => Some(p.as_path()),
            _ => None,
        }
    }

    /// The window `[first, first + h)` of the lines a made text plugin returned already colored,
    /// as the host draws them. `None` unless the open view is such.
    pub(super) fn made_text_window(&self, first: usize, h: usize) -> Option<Vec<Line<'static>>> {
        if !self.text_plugin_is_generated() {
            return None;
        }
        match &self.text_plugin_view()?.source {
            TextSource::Lines(lines) => Some(
                lines
                    .iter()
                    .skip(first)
                    .take(h)
                    .cloned()
                    .map(ratatui_line)
                    .collect(),
            ),
            _ => None,
        }
    }

    /// A made text plugin's view landed: its text becomes the file the reader windows (a private
    /// temp file the tab owns; a file the plugin names is read in place), and the window is opened
    /// on it afresh. The colors stay with the view.
    ///
    /// The worker wrote the text (`made`, `plugin_host::write_made_text`); the UI thread only takes
    /// the path, so a plugin that returns a large string cannot stop the interface.
    pub(super) fn made_text_landed(&mut self, made: Option<std::io::Result<MadeText>>) {
        let has_text = match self.plugin.as_ref().and_then(|p| p.view.as_ref()) {
            Some(View::Text(t)) => !matches!(t.source, TextSource::File(_)),
            _ => return,
        };
        if has_text {
            self.clear_command_out();
            match made.unwrap_or_else(|| Err(std::io::Error::other("no text was written"))) {
                Ok(file) => self.tab.command_out = file.take(),
                Err(e) => {
                    self.flash = Some(format!(
                        "{}{e}",
                        tr(self.lang, crate::i18n::Msg::OperationFailed)
                    ));
                    return;
                }
            }
        }
        self.setup_windowed();
    }

    /// The styler of the open `code` show, once its first result has landed (and only when the
    /// plugin chose to color: it returns none with `syntax_highlight` off).
    fn code_styler(&mut self) -> Option<&mut Box<dyn LineStyler>> {
        if !self.shows_text_plugin() {
            return None;
        }
        let show = self.plugin.as_mut()?;
        match show.view.as_mut()? {
            View::Text(TextView { styler, .. }) => styler.as_mut(),
            _ => None,
        }
    }

    /// Whether the `code` plugin has not answered yet (its first result has not landed). The
    /// window is plain and uncached meanwhile; once it has answered, or failed, the window is
    /// colored: by the plugin's styler, or (a failed plugin) by the same styler built here.
    pub(super) fn text_plugin_pending(&self) -> bool {
        self.shows_text_plugin()
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
