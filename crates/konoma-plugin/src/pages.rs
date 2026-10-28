//! The page state both hosts keep for a view made of separate pages (a PDF's pages, a workbook's
//! sheets): what the page keys do, the page count a layout reports, the position that is shown.
//!
//! This is the arithmetic of the konoma host (`PerTab::page`, `PerTab::page_count`) and of the
//! standalone app; both call it, so the two cannot count, clamp or stop at the ends differently.
//! Pages are counted from 1 here (the host's way); a [`crate::ViewReq`] counts from 0.

use crate::view::{Layout, PageLayout};

/// The page count a layout reports for pages that are fetched one at a time: `None` when the
/// layout is not made of separate pages (the host's page state is left alone or cleared by the
/// caller), `Some(None)` when the plugin does not say how many there are, `Some(Some(n))` else.
pub fn count_of(layout: &Layout) -> Option<Option<u32>> {
    match &layout.pages {
        Some(PageLayout::Separate { count, .. }) => Some(count.and_then(|n| u32::try_from(n).ok())),
        _ => None,
    }
}

/// Whether the page keys have pages to turn: a known count of more than one.
pub fn can_turn(count: Option<u32>) -> bool {
    matches!(count, Some(n) if n > 1)
}

/// `page` kept inside `1..=count` (a count of 0 is read as 1).
pub fn clamp(page: u32, count: u32) -> u32 {
    page.clamp(1, count.max(1))
}

/// The page a key asks for from `page`: the next one for `dir >= 0`, the previous one else. Not yet
/// clamped to the pages there are ([`goto`] does that).
pub fn target(page: u32, dir: i32) -> u32 {
    if dir >= 0 {
        page.saturating_add(1)
    } else {
        page.saturating_sub(1)
    }
}

/// Where a jump to `to` from `page` lands, or `None` when nothing changes: there are no pages to
/// turn, or the page clamped to `1..=count` is the one already shown.
pub fn goto(page: u32, count: Option<u32>, to: u32) -> Option<u32> {
    if !can_turn(count) {
        return None;
    }
    let to = count.map_or(to, |n| to.clamp(1, n));
    (to != page).then_some(to)
}

/// Where the page key `dir` (`J` is 1, `K` is -1) takes the view from `page`, or `None` when it
/// stays: at an end the view stops (no wrap), and one page has nothing to turn.
pub fn turn(page: u32, count: Option<u32>, dir: i32) -> Option<u32> {
    goto(page, count, target(page, dir))
}

/// `(current, total)` of the pages, or `None` when the count is not known. The current page is
/// held inside the count.
pub fn indicator(page: u32, count: Option<u32>) -> Option<(u32, u32)> {
    let total = count?;
    Some((page.min(total), total))
}

/// The position as shown: `2/5`.
pub fn indicator_text(cur: u32, total: u32) -> String {
    format!("{cur}/{total}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_count_is_read_only_from_separate_pages() {
        let sep = |count| Layout {
            pages: Some(PageLayout::Separate {
                count,
                titles: Vec::new(),
            }),
            ..Layout::single()
        };
        assert_eq!(count_of(&sep(Some(3))), Some(Some(3)));
        assert_eq!(count_of(&sep(None)), Some(None));
        assert_eq!(count_of(&Layout::single()), None);
        let anchored = Layout {
            pages: Some(PageLayout::Anchored {
                heading_level: 1,
                count: 4,
            }),
            ..Layout::single()
        };
        assert_eq!(count_of(&anchored), None);
    }

    #[test]
    fn a_page_turn_stops_at_both_ends_and_does_nothing_for_one_page() {
        assert_eq!(turn(1, Some(3), 1), Some(2));
        assert_eq!(turn(2, Some(3), 1), Some(3));
        assert_eq!(turn(3, Some(3), 1), None);
        assert_eq!(turn(3, Some(3), -1), Some(2));
        assert_eq!(turn(1, Some(3), -1), None);
        assert_eq!(turn(1, Some(1), 1), None);
        assert_eq!(turn(1, Some(1), -1), None);
        assert_eq!(turn(1, None, 1), None);
        assert_eq!(turn(1, Some(0), 1), None);
        // A page past the end is brought back to the last one by the jump.
        assert_eq!(goto(9, Some(3), 9), Some(3));
        assert_eq!(goto(3, Some(3), 9), None);
        assert_eq!(goto(2, Some(3), 0), Some(1));
        // `dir` 0 reads as forward, as the host's page key does.
        assert_eq!(target(2, 0), 3);
        assert_eq!(target(2, -5), 1);
        assert_eq!(target(0, -1), 0);
    }

    #[test]
    fn the_position_is_held_inside_the_count() {
        assert_eq!(indicator(2, Some(5)), Some((2, 5)));
        assert_eq!(indicator(9, Some(5)), Some((5, 5)));
        assert_eq!(indicator(1, None), None);
        assert_eq!(indicator_text(2, 5), "2/5");
        assert_eq!(clamp(0, 3), 1);
        assert_eq!(clamp(7, 3), 3);
        assert_eq!(clamp(4, 0), 1);
    }
}
