//! The two footer lines that take the place of the key hints for a moment: the which-key line of
//! an open menu (`y`, `Space`, `,`) and the notification (flash) line. konoma and the standalone
//! plugin app draw them the same way; what they say comes from the host.

use ratatui::style::Stylize;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::help_panel::fit_tokens;

/// The which-key line of an open menu: the bold heading `<title> ▸ ` and the dim candidates
/// `key:label`, as many (from the first) as fit in `width` cells; when some are cut ` …` ends the
/// line. `items` are `(key as typed, label)`. An empty menu gives the heading alone.
pub fn which_key_spans(title: &str, items: &[(String, String)], width: u16) -> Vec<Span<'static>> {
    let prefix = format!("{title} ▸ ");
    let room = width.saturating_sub(prefix.width() as u16);
    let tokens: Vec<String> = items.iter().map(|(k, l)| format!("{k}:{l}")).collect();
    vec![
        Span::from(prefix).bold(),
        Span::from(fit_tokens(&tokens, room)).dim(),
    ]
}

/// The notification line (`copied: …`): the message in bold. It is not cut here; the terminal
/// clips the row, and the message is cleared by the next key.
pub fn flash_spans(msg: &str) -> Vec<Span<'static>> {
    vec![Span::from(msg.to_string()).bold()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn items(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .collect()
    }

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn items_are_listed_in_order_with_the_heading() {
        let s = which_key_spans("copy", &items(&[("p", "path"), ("n", "name")]), 80);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].content.as_ref(), "copy ▸ ");
        assert!(s[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(s[1].content.as_ref(), "p:path  n:name");
        assert!(s[1].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_narrow_width_keeps_the_first_items_and_ends_with_an_ellipsis() {
        let it = items(&[("a", "aaa"), ("b", "bbb"), ("c", "ccc")]);
        // prefix "m ▸ " is 4 cells; 12 cells left: "a:aaa  b:bbb" (12) + " …" does not fit.
        let s = which_key_spans("m", &it, 16);
        assert_eq!(s[1].content.as_ref(), "a:aaa …");
        assert!(!text(&s).contains("ccc"));
        // Nothing fits after the heading: heading alone, no panic.
        let s = which_key_spans("menu", &it, 3);
        assert_eq!(s[1].content.as_ref(), "");
    }

    #[test]
    fn full_width_labels_are_counted_in_cells() {
        let it = items(&[("p", "パスをコピー"), ("n", "名前")]);
        // heading 4 cells ("m ▸ "); "p:パスをコピー" is 14 cells and " …" needs 2 more.
        let s = which_key_spans("m", &it, 4 + 16);
        assert_eq!(s[1].content.as_ref(), "p:パスをコピー …");
        let wide = which_key_spans("m", &it, 80);
        assert_eq!(wide[1].content.as_ref(), "p:パスをコピー  n:名前");
        // A full-width title counts as two cells per character.
        let s = which_key_spans("コピー", &it, 80);
        assert_eq!(s[0].content.as_ref(), "コピー ▸ ");
    }

    #[test]
    fn an_empty_menu_is_the_heading_alone() {
        let s = which_key_spans("copy", &[], 80);
        assert_eq!(text(&s), "copy ▸ ");
    }

    #[test]
    fn a_long_flash_is_kept_whole_and_bold() {
        let long = "copied: ".to_string() + &"長い名前".repeat(40);
        let s = flash_spans(&long);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].content.as_ref(), long);
        assert!(s[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(flash_spans("")[0].content.as_ref(), "");
    }
}
