//! The `[ui]` display settings the standalone app honours, read from the shared config the way
//! konoma reads them: wrapping, the document's colors, icons and tab stop.

use konoma_host_core::shared::{SharedConfig, SharedTheme};
use konoma_widgets::markdown::CodeStyle;

/// How text and documents are drawn (`[ui] wrap`, `icons`, `tab_width` and `[ui.theme]`).
/// The default is the default config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    /// Soft-wrap text and documents. Off, they scroll sideways (`h` `l` `0` `$`).
    pub wrap: bool,
    /// Icon glyphs in a document.
    pub icons: bool,
    /// The tab stop of code blocks.
    pub tab_width: usize,
    pub theme: SharedTheme,
}

impl Default for Display {
    fn default() -> Self {
        Self::of(&SharedConfig::default())
    }
}

impl Display {
    /// The display settings of a loaded config.
    pub fn of(cfg: &SharedConfig) -> Self {
        Self {
            wrap: cfg.ui.wrap,
            icons: cfg.ui.icons,
            tab_width: cfg.ui.tab_width,
            theme: cfg.ui.theme.clone(),
        }
    }

    /// `[ui.theme] bg`: the background under the whole screen (`None`: the terminal's own).
    pub fn bg(&self) -> Option<ratatui::style::Color> {
        konoma_widgets::colors::theme_bg(&self.theme.bg)
    }

    /// The code decoration of a document (the one function konoma builds it with).
    pub fn code_style(&self) -> CodeStyle {
        konoma_widgets::colors::code_style(
            (
                &self.theme.code_bg,
                &self.theme.code_label_align,
                &self.theme.code_label_bg,
            ),
            self.tab_width,
            self.wrap,
        )
    }
}
