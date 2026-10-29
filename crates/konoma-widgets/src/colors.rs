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

/// Resolves a color string. `"none"`/empty -> None. Parse success -> that color. Parse failure
/// -> `fallback`.
pub fn parse_color_opt(s: &str, fallback: Option<Color>) -> Option<Color> {
    use std::str::FromStr;
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return None;
    }
    match Color::from_str(s) {
        Ok(c) => Some(c),
        Err(_) => fallback,
    }
}

/// `[ui.theme] bg`: `"none"`/empty/invalid -> None (the terminal's own background).
pub fn theme_bg(bg: &str) -> Option<Color> {
    parse_color_opt(bg, None)
}

/// `[ui.theme] code_bg`: `"none"`/empty -> no background; a parse failure -> the default color.
pub fn theme_code_bg(code_bg: &str) -> Option<Color> {
    parse_color_opt(code_bg, Some(DEFAULT_CODE_BG))
}

/// `[ui.theme] code_label_align`: left only when `"left"` (any case); otherwise right.
pub fn theme_code_label_right(align: &str) -> bool {
    !align.trim().eq_ignore_ascii_case("left")
}

/// `[ui.theme] code_label_bg`: `"auto"` is a brightened `code_bg`; `"none"` is no background.
pub fn theme_code_label_bg(label_bg: &str, code_bg: &str) -> Option<Color> {
    if label_bg.trim().eq_ignore_ascii_case("auto") {
        return theme_code_bg(code_bg).map(lighten);
    }
    parse_color_opt(label_bg, None)
}

/// The code decoration of a document from the `[ui.theme]` strings, `ui.tab_width` and `ui.wrap`:
/// the one place both hosts turn the settings into a [`crate::markdown::CodeStyle`].
pub fn code_style(
    (code_bg, label_align, label_bg): (&str, &str, &str),
    tab_width: usize,
    wrap: bool,
) -> crate::markdown::CodeStyle {
    crate::markdown::CodeStyle {
        bg: theme_code_bg(code_bg),
        label_bg: theme_code_label_bg(label_bg, code_bg),
        label_right: theme_code_label_right(label_align),
        tab_width,
        wrap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_colors_resolve_as_written() {
        assert_eq!(theme_bg("none"), None);
        assert_eq!(theme_bg("nonsense"), None);
        assert_eq!(theme_bg("#102030"), Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme_code_bg("none"), None);
        assert_eq!(theme_code_bg("nonsense"), Some(DEFAULT_CODE_BG));
        assert_eq!(theme_code_bg("#102030"), Some(Color::Rgb(16, 32, 48)));
    }

    #[test]
    fn the_label_aligns_right_unless_it_says_left() {
        assert!(theme_code_label_right("right"));
        assert!(theme_code_label_right("sideways"));
        assert!(!theme_code_label_right("left"));
        assert!(!theme_code_label_right(" LEFT "));
    }

    #[test]
    fn the_label_background_is_auto_none_or_a_color() {
        let lighter = Some(lighten(Color::Rgb(16, 32, 48)));
        assert_eq!(theme_code_label_bg("auto", "#102030"), lighter);
        assert_eq!(theme_code_label_bg(" AUTO ", "#102030"), lighter);
        assert_eq!(theme_code_label_bg("auto", "none"), None);
        assert_eq!(theme_code_label_bg("none", "#102030"), None);
        assert_eq!(theme_code_label_bg("nonsense", "#102030"), None);
        assert_eq!(
            theme_code_label_bg("#405060", "#102030"),
            Some(Color::Rgb(64, 80, 96))
        );
    }

    #[test]
    fn the_code_style_carries_every_setting() {
        let s = code_style(("#102030", "left", "#405060"), 8, false);
        assert_eq!(s.bg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(s.label_bg, Some(Color::Rgb(64, 80, 96)));
        assert!(!s.label_right);
        assert_eq!((s.tab_width, s.wrap), (8, false));
        let d = code_style(("#2b303b", "right", "auto"), 4, true);
        assert_eq!(d.bg, Some(DEFAULT_CODE_BG));
        assert_eq!(d.label_bg, Some(lighten(DEFAULT_CODE_BG)));
        assert!(d.label_right);
        assert_eq!((d.tab_width, d.wrap), (4, true));
    }
}
