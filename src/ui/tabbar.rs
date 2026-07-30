// Tab bar rendering (M4 / FR-5).
//
// Lays out multiple tree contexts as tabs along the top edge. Whether they're shown is decided by
// ui::render based on the `ui.tabbar` config ("always" | "auto" | "hidden"); this module only draws.
// The active tab is reversed + bold. Each tab reads "number:directory name".
//
// When it doesn't fit the width, it draws only a visible window that grows **centered on the
// active tab**, alternating sides, and shows the number of tabs hidden at each edge as "‹n / n›"
// (guards against the regression where the active tab becomes invisible — the full tab list is `T`).

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.active_tab_index();
    let n = app.tab_count();
    let chips: Vec<String> = (0..n)
        .map(|i| format!(" {}:{} ", i + 1, app.tab_label(i)))
        .collect();
    let widths: Vec<usize> = chips.iter().map(|c| c.width()).collect();
    let (s, e) = visible_range(&widths, active, area.width as usize);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if s > 0 {
        spans.push(Span::from(format!("‹{} ", s)).dim());
    }
    for (k, chip) in chips.into_iter().enumerate().take(e).skip(s) {
        if k > s {
            spans.push(Span::from(" "));
        }
        if k == active {
            spans.push(Span::from(chip).reversed().bold());
        } else {
            spans.push(Span::from(chip).dim());
        }
    }
    if e < n {
        spans.push(Span::from(format!(" {}›", n - e)).dim());
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Width taken by an overflow marker (`‹12 ` / ` 12›`) for `hidden` tabs; 0 when nothing is hidden.
fn marker_width(hidden: usize) -> usize {
    if hidden == 0 {
        0
    } else {
        hidden.to_string().len() + 2
    }
}

/// Pick the visible tab range `[s, e)` for `avail` columns. Everything is shown when it fits;
/// otherwise the window grows from the active tab, alternating sides (≒ centered), reserving
/// room for the `‹n` / `n›` overflow markers of whichever sides stay hidden.
fn visible_range(widths: &[usize], active: usize, avail: usize) -> (usize, usize) {
    let n = widths.len();
    if n == 0 {
        return (0, 0);
    }
    let total: usize = widths.iter().sum::<usize>() + n.saturating_sub(1);
    if total <= avail {
        return (0, n);
    }
    let active = active.min(n - 1);
    let fits = |s: usize, e: usize| -> bool {
        let w: usize = widths[s..e].iter().sum::<usize>()
            + (e - s - 1)
            + marker_width(s)
            + marker_width(n - e);
        w <= avail
    };
    let (mut s, mut e) = (active, active + 1);
    loop {
        // Center: try the side with less fill first; if it doesn't fit, try the other side; if
        // neither fits, settle.
        let prefer_right = (e - active) <= (active - s);
        let first = if prefer_right && e < n {
            Some((s, e + 1))
        } else if s > 0 {
            Some((s - 1, e))
        } else if e < n {
            Some((s, e + 1))
        } else {
            None
        };
        let second = if first == Some((s, e + 1)) {
            (s > 0).then(|| (s - 1, e))
        } else {
            (e < n).then(|| (s, e + 1))
        };
        match (first, second) {
            (Some((fs, fe)), _) if fits(fs, fe) => {
                s = fs;
                e = fe;
            }
            (_, Some((ss, se))) if fits(ss, se) => {
                s = ss;
                e = se;
            }
            _ => break,
        }
    }
    (s, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_keeps_active_visible_and_centers() {
        // Everything fits: show it all as-is.
        let w = vec![8, 8, 8];
        assert_eq!(visible_range(&w, 1, 80), (0, 3));
        // Overflow: a window containing the active tab (4) is chosen, centered, with marker width
        // reserved on both ends.
        let w = vec![10; 9]; // each 10 columns + separator → 98 columns total
        let (s, e) = visible_range(&w, 4, 40);
        assert!((s..e).contains(&4), "アクティブは必ず可視: {s}..{e}");
        assert!(s > 0 && e < 9, "両側に隠れタブ: {s}..{e}");
        let used: usize =
            w[s..e].iter().sum::<usize>() + (e - s - 1) + marker_width(s) + marker_width(9 - e);
        assert!(used <= 40, "マーカー込みで幅内: {used}");
        // Edge: when the active tab is first, it extends right with no left marker.
        let (s, e) = visible_range(&w, 0, 40);
        assert_eq!(s, 0);
        assert!(e < 9);
        // Edge: when the active tab is last, it extends left with no right marker.
        let (s, e) = visible_range(&w, 8, 40);
        assert_eq!(e, 9);
        assert!(s > 0);
        // Extremely narrow: still returns at least the active tab alone (letting the renderer
        // clip it is acceptable).
        let (s, e) = visible_range(&w, 4, 8);
        assert_eq!((s, e), (4, 5));
    }
}
