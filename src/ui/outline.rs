//! Centered popup for the heading outline (`o` in a Markdown preview). One row per heading, indented
//! by level, with the selected row reversed. `j`/`k`/`g`/`G` move, `Enter` jumps the preview to the
//! heading, `o`/`q`/`Esc` close. Follows the tab-list popup layout.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::list_filter::ListId;
use crate::app::App;
use crate::i18n::tr;

/// A list this long or shorter measures every row for the popup width (the old behaviour, exact for
/// any ordinary outline or page list); a longer one is sized by the rows in view instead.
const FULL_WIDTH_SCAN: usize = 2000;

/// What `build` needs to know about the list; borrowed from the app so it can be tested alone.
pub(crate) struct Rows<'a> {
    /// `(level, text)` of every entry (the filter does not remove any from here).
    pub items: &'a [(u8, String)],
    /// The entries the `/` filter leaves shown (sorted indices), or `None` for all.
    pub shown: Option<&'a [usize]>,
    /// The selected entry (an index into `items`).
    pub sel: usize,
    /// The entry that marks where the preview is (the page list only).
    pub current: Option<usize>,
    /// The key-hint line under the rows.
    pub hint: &'a str,
}

/// The popup: only the lines in view, and its size.
pub(crate) struct Built {
    pub lines: Vec<Line<'static>>,
    pub w: u16,
    pub h: u16,
    /// The content width the popup was sized by (before the border and the clamp).
    pub content_w: usize,
}

/// The columns a row takes: the same as `Line::width` of what `row_text` makes, without making it.
fn row_width(level: u8, text: &str, mark: char) -> usize {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    1 + 2 * (level.saturating_sub(1) as usize) + mark.width().unwrap_or(0) + 1 + text.width()
}

fn row_text(level: u8, text: &str, mark: char) -> String {
    // Indent by level (H1 flush-left); a bullet marks the heading.
    let indent = "  ".repeat(level.saturating_sub(1) as usize);
    format!(" {indent}{mark} {text}")
}

fn mark_of(current: Option<usize>, i: usize) -> char {
    // The page list marks the page the preview is on.
    if current == Some(i) {
        '▶'
    } else {
        '\u{2022}'
    }
}

/// Builds only the rows that are drawn. The popup is `rows + blank + hint` lines tall (capped to the
/// screen) and scrolls so the selected row is the last visible one at most; all counting is
/// `usize`, so a list of any length cannot overflow a row counter.
///
/// Width: up to `FULL_WIDTH_SCAN` shown rows are all measured. A longer list is sized by the rows in
/// view, and `width_floor` (the widest such window so far in this opening) keeps the popup from
/// shrinking while it scrolls, so it only ever grows (at most to the cap of 70).
pub(crate) fn build(rows: &Rows, area: Rect, width_floor: usize) -> Built {
    let n_shown = rows.shown.map_or(rows.items.len(), <[usize]>::len);
    let item_at = |pos: usize| rows.shown.map_or(pos, |v| v[pos]);
    let total = n_shown + 2; // + blank + hint
    let h = (total + 2)
        .min(area.height.saturating_sub(2) as usize)
        .max(3);
    let inner_h = h.saturating_sub(2);
    // The selected row's place among the shown rows (0 when the filter hides it).
    let sel_line = match rows.shown {
        None if rows.sel < n_shown => rows.sel,
        None => 0,
        Some(v) => v.binary_search(&rows.sel).unwrap_or(0),
    };
    let max_scroll = total.saturating_sub(inner_h);
    let scroll = sel_line
        .saturating_sub(inner_h.saturating_sub(1))
        .min(max_scroll);
    let end = (scroll + inner_h).min(total);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(end.saturating_sub(scroll));
    let mut window_w = 0usize;
    for pos in scroll..end {
        if pos < n_shown {
            let i = item_at(pos);
            let (level, text) = &rows.items[i];
            let row = row_text(*level, text, mark_of(rows.current, i));
            let line = Line::from(row);
            window_w = window_w.max(line.width());
            lines.push(if i == rows.sel { line.reversed() } else { line });
        } else if pos == n_shown {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(rows.hint.to_string()).dim());
        }
    }
    let hint_w = Line::from(rows.hint).width();
    let content_w = if n_shown <= FULL_WIDTH_SCAN {
        (0..n_shown)
            .map(|pos| {
                let i = item_at(pos);
                let (level, text) = &rows.items[i];
                row_width(*level, text, mark_of(rows.current, i))
            })
            .max()
            .unwrap_or(0)
            .max(hint_w)
    } else {
        window_w.max(hint_w).max(width_floor)
    };
    // Width = the widest line (border included), capped at 70; fits within the screen.
    let content = if content_w == 0 { 20 } else { content_w };
    let w = (content + 2)
        .clamp(24, 70)
        .min(area.width.saturating_sub(2) as usize);
    Built {
        lines,
        w: w as u16,
        h: h as u16,
        content_w,
    }
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.outline_rows();
    let shown = app.list_visible(ListId::Outline);
    let current = app.outline_current_row();
    let (title, actions) = if current.is_some() {
        (
            crate::i18n::Msg::PageListTitle,
            crate::i18n::Msg::PageListActions,
        )
    } else {
        (
            crate::i18n::Msg::OutlineTitle,
            crate::i18n::Msg::OutlineActions,
        )
    };
    // A query shows as " /<query>" after the title.
    let q = app.list_filter_query(ListId::Outline);
    let floor = app.outline_width_floor(q);
    let built = build(
        &Rows {
            items: &items,
            shown: shown.as_deref(),
            sel: app.outline_sel(),
            current,
            hint: tr(app.lang, actions),
        },
        area,
        floor,
    );
    app.outline_width_seen(q, built.content_w);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(built.w)) / 2,
        y: area.y + (area.height.saturating_sub(built.h)) / 2,
        width: built.w,
        height: built.h,
    };
    let title = if q.is_empty() {
        tr(app.lang, title).to_string()
    } else {
        format!("{} /{q} ", tr(app.lang, title).trim_end())
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    // The lines are already the visible window, so nothing scrolls here.
    let para = Paragraph::new(built.lines)
        .block(block)
        .alignment(Alignment::Left);

    konoma_widgets::overlay::clear_overlay(frame.buffer_mut(), popup);
    frame.render_widget(para, popup);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<(u8, String)> {
        (0..n).map(|i| (1, format!("title {i}"))).collect()
    }

    fn rows<'a>(
        items: &'a [(u8, String)],
        shown: Option<&'a [usize]>,
        sel: usize,
        current: Option<usize>,
    ) -> Rows<'a> {
        Rows {
            items,
            shown,
            sel,
            current,
            hint: "j/k move  Enter go",
        }
    }

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };

    #[test]
    fn row_width_equals_the_width_of_the_line_it_stands_for() {
        // Mixed levels, ASCII, CJK (2 columns each) and an emoji.
        for (level, t) in [
            (1u8, "abc"),
            (3, "日本語の見出し"),
            (2, "mix 日本 ok"),
            (1, ""),
            (6, "🙂 smile"),
        ] {
            for mark in ['▶', '•'] {
                assert_eq!(
                    row_width(level, t, mark),
                    Line::from(row_text(level, t, mark)).width(),
                    "{level} {t:?} {mark}"
                );
            }
        }
    }

    #[test]
    fn a_hundred_thousand_rows_build_only_the_window() {
        let it = items(100_000);
        // Selection at the end: the last row is in view and selected.
        let b = build(&rows(&it, None, 99_999, None), AREA, 0);
        assert_eq!(b.h, 28, "capped to the screen (30 - 2)");
        assert_eq!(b.lines.len(), 26, "the window, not the list");
        let texts: Vec<String> = b.lines.iter().map(text).collect();
        assert!(texts.iter().any(|t| t.contains("title 99999")));
        // The scroll rule is unchanged: the selected row is the last one in view.
        assert!(texts.last().unwrap().contains("title 99999"));
        let selected: Vec<&Line> = b
            .lines
            .iter()
            .filter(|l| {
                l.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
            .collect();
        assert_eq!(selected.len(), 1);
        assert!(text(selected[0]).contains("title 99999"));
        // First page: the top rows, selected first.
        let b = build(&rows(&it, None, 0, None), AREA, 0);
        let texts: Vec<String> = b.lines.iter().map(text).collect();
        assert!(texts[0].contains("title 0"));
        assert!(texts[25].contains("title 25"));
        assert!(!texts.iter().any(|t| t.contains("title 99999")));
        // The middle: the selected row is the last one in view (the scroll rule is unchanged).
        let b = build(&rows(&it, None, 50_000, None), AREA, 0);
        let texts: Vec<String> = b.lines.iter().map(text).collect();
        assert!(texts[25].contains("title 50000"));
        assert!(texts[0].contains("title 49975"));
    }

    #[test]
    fn a_list_taller_than_u16_does_not_wrap_the_row_counter() {
        // 70,000 rows (> u16::MAX); the old `as u16` made the scroll/height wrap.
        let it = items(70_000);
        let b = build(&rows(&it, None, 69_999, None), AREA, 0);
        assert_eq!(b.h, 28);
        assert!(text(&b.lines[b.lines.len() - 1]).contains("title 69999"));
        // The marked page (current) shows its marker far past 65,535.
        let b = build(&rows(&it, None, 66_000, Some(66_000)), AREA, 0);
        assert!(b.lines.iter().any(|l| text(l).contains("▶ title 66000")));
    }

    #[test]
    fn a_short_list_is_as_before_every_row_the_hint_and_the_widest_row_sets_the_width() {
        let mut it = items(5);
        it[4].1 = "a much longer title than the rest of them".into();
        let b = build(&rows(&it, None, 0, None), AREA, 0);
        assert_eq!(b.lines.len(), 7, "5 rows + blank + hint");
        assert_eq!(b.h, 9);
        // " • " + 41 columns of text, + border.
        assert_eq!(b.content_w, row_width(1, &it[4].1, '•'));
        assert_eq!(b.w as usize, b.content_w + 2);
        // Even when the long row is scrolled out of a short screen, the width counts it.
        let small = Rect { height: 7, ..AREA };
        let b = build(&rows(&it, None, 0, None), small, 0);
        assert_eq!(b.h, 5);
        assert_eq!(b.w as usize, row_width(1, &it[4].1, '•') + 2);
    }

    #[test]
    fn a_long_list_is_sized_by_its_window_and_never_shrinks() {
        let mut it = items(5000);
        it[100].1 = "w".repeat(50);
        // Window around row 100 (selected there): wide.
        let at100 = build(&rows(&it, None, 100, None), AREA, 0);
        assert!(at100.content_w >= row_width(1, &it[100].1, '•'));
        // Far from it, the window alone is narrow ...
        let far = build(&rows(&it, None, 3000, None), AREA, 0);
        assert!(far.content_w < at100.content_w);
        // ... but with the floor of what was seen, the popup keeps its width.
        let kept = build(&rows(&it, None, 3000, None), AREA, at100.content_w);
        assert_eq!(kept.w, at100.w);
        // The cap holds: a huge floor cannot exceed 70 columns or the screen.
        let capped = build(&rows(&it, None, 3000, None), AREA, 10_000);
        assert_eq!(capped.w, 70);
        let narrow = Rect { width: 40, ..AREA };
        assert_eq!(build(&rows(&it, None, 0, None), narrow, 10_000).w, 38);
    }

    #[test]
    fn cjk_rows_are_measured_in_columns_not_characters() {
        let mut it = items(3000);
        // 10 wide characters = 20 columns (not 10), the widest row in the window.
        it[0].1 = "日本語日本語日本語日".into();
        let b = build(&rows(&it, None, 0, None), AREA, 0);
        assert_eq!(b.content_w, 1 + 1 + 1 + 20, "' ' + mark + ' ' + 20 columns");
        assert!(text(&b.lines[0]).contains("日本語日本語日本語日"));
        // The same rule in the exact (short list) path.
        let short = vec![(2u8, "日本語日本語日本語日".to_string())];
        let b = build(&rows(&short, None, 0, None), AREA, 0);
        assert_eq!(b.content_w, 1 + 2 + 1 + 1 + 20);
    }

    #[test]
    fn a_filter_maps_window_positions_to_the_shown_entries() {
        let it = items(100_000);
        let shown: Vec<usize> = (0..100_000).step_by(1000).collect(); // 100 rows
        let b = build(&rows(&it, Some(&shown), 99_000, None), AREA, 0);
        let texts: Vec<String> = b.lines.iter().map(text).collect();
        assert!(texts.iter().any(|t| t.contains("title 99000")));
        assert!(!texts.iter().any(|t| t.contains("title 99001")));
        // A selection the filter hides draws no highlight and starts at the top.
        let b = build(&rows(&it, Some(&shown), 5, None), AREA, 0);
        assert!(text(&b.lines[0]).contains("title 0"));
        // Nothing shown: just the blank and the hint.
        let none: Vec<usize> = Vec::new();
        let b = build(&rows(&it, Some(&none), 0, None), AREA, 0);
        assert_eq!(b.lines.len(), 2);
    }

    #[test]
    fn an_empty_list_and_a_tiny_screen_do_not_panic() {
        let none: Vec<(u8, String)> = Vec::new();
        let b = build(&rows(&none, None, 0, None), AREA, 0);
        assert_eq!(b.lines.len(), 2);
        let tiny = Rect {
            width: 3,
            height: 2,
            ..AREA
        };
        let it = items(10);
        let b = build(&rows(&it, None, 9, None), tiny, 0);
        assert!(b.h >= 3 && b.lines.len() <= 1);
    }
}
