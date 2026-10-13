//! Colors the document renderer and the host's theme settings share.

use ratatui::style::Color;

/// Default code background color for Markdown (dark slate). Used when `ui.theme.code_bg` is unspecified.
pub const DEFAULT_CODE_BG: Color = Color::Rgb(43, 48, 59);

/// Brightens a background color slightly (to distinguish the language badge from body code). Non-Rgb is left as-is.
pub fn lighten(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            r.saturating_add(27),
            g.saturating_add(30),
            b.saturating_add(40),
        ),
        other => other,
    }
}
