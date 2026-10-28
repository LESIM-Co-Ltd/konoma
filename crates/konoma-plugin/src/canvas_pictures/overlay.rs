//! What the canvas overlay draws in one frame, as data: where a picture lands inside the content
//! area ([`visible_rect`]), how much picture area one canvas may cover ([`MAX_PICTURE_AREA_FACTOR`]),
//! and the signature of what is where ([`OverlaySig`]). Host-neutral: the hosts paint, this decides.

use ratatui::layout::Rect;

/// The most the pictures of one canvas may cover, as a multiple of the canvas area, counting every
/// placement (overlapping ones too): beyond it the placements, in the plugin's order, keep their
/// cells instead of being encoded. An honest canvas covers a part of its area; the bound only
/// stops a plugin that stacks many pictures over all of it from making the host encode (and a
/// terminal store) hundreds of screens of pixels.
pub const MAX_PICTURE_AREA_FACTOR: u64 = 4;

/// The signature of what the canvas overlay drew in one frame: the area, and for each picture its
/// id and where it was drawn. The run loop clears the screen once when it changes (debris of a
/// picture that moved or left), so it follows **what is where**, never the content of a picture.
pub struct OverlaySig(std::collections::hash_map::DefaultHasher);

impl OverlaySig {
    pub fn new(inner: Rect) -> OverlaySig {
        use std::hash::Hash;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (inner.x, inner.y, inner.width, inner.height).hash(&mut h);
        OverlaySig(h)
    }

    pub fn add(&mut self, id: u64, at: Rect) {
        use std::hash::Hash;
        (id, at.x, at.y, at.width, at.height).hash(&mut self.0);
    }

    pub fn finish(&self) -> u64 {
        use std::hash::Hasher;
        self.0.finish()
    }
}

/// Where the picture `r` (in canvas cells) lands inside `inner`, or `None` when none of it is
/// shown: it starts outside, or reaches past the right edge.
pub fn visible_rect(inner: Rect, r: crate::view::CellRect) -> Option<Rect> {
    if r.cols == 0 || r.rows == 0 || r.x >= inner.width || r.y >= inner.height {
        return None;
    }
    if r.x as u32 + r.cols as u32 > inner.width as u32 {
        return None;
    }
    let rows = r.rows.min(inner.height - r.y);
    Some(Rect {
        x: inner.x + r.x,
        y: inner.y + r.y,
        width: r.cols,
        height: rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::CellRect;

    fn sig(inner: Rect, items: &[(u64, Rect)]) -> u64 {
        let mut s = OverlaySig::new(inner);
        for (id, r) in items {
            s.add(*id, *r);
        }
        s.finish()
    }

    #[test]
    fn the_overlay_signature_follows_every_input() {
        let area = Rect::new(1, 2, 30, 10);
        let at = Rect::new(5, 3, 8, 4);
        let base = sig(area, &[(1, at)]);
        assert_eq!(base, sig(area, &[(1, at)]), "stable");
        assert_ne!(base, sig(Rect::new(2, 2, 30, 10), &[(1, at)]), "area x");
        assert_ne!(base, sig(Rect::new(1, 3, 30, 10), &[(1, at)]), "area y");
        assert_ne!(base, sig(Rect::new(1, 2, 31, 10), &[(1, at)]), "area width");
        assert_ne!(
            base,
            sig(Rect::new(1, 2, 30, 11), &[(1, at)]),
            "area height"
        );
        assert_ne!(
            base,
            sig(area, &[(2, at)]),
            "another picture in the same place"
        );
        assert_ne!(base, sig(area, &[(1, Rect::new(6, 3, 8, 4))]), "x");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 4, 8, 4))]), "y");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 3, 9, 4))]), "width");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 3, 8, 3))]), "height");
        assert_ne!(base, sig(area, &[]), "gone");
        assert_ne!(base, sig(area, &[(1, at), (2, at)]), "another one");
    }

    const INNER: Rect = Rect {
        x: 3,
        y: 2,
        width: 20,
        height: 10,
    };

    #[test]
    fn visible_rect_offsets_into_the_area() {
        assert_eq!(
            visible_rect(INNER, CellRect::new(4, 1, 5, 3)),
            Some(Rect::new(7, 3, 5, 3))
        );
    }

    #[test]
    fn visible_rect_clips_the_bottom_to_a_band_and_keeps_the_top() {
        // Starts at row 8 of 10, 4 rows tall: the top 2 rows are shown.
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 8, 5, 4)),
            Some(Rect::new(3, 10, 5, 2))
        );
        // Ends exactly at the edge: whole.
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 6, 5, 4)),
            Some(Rect::new(3, 8, 5, 4))
        );
    }

    #[test]
    fn visible_rect_skips_what_starts_outside_or_does_not_fit_across() {
        assert_eq!(
            visible_rect(INNER, CellRect::new(20, 0, 2, 2)),
            None,
            "starts at the right edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 10, 2, 2)),
            None,
            "starts at the bottom edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(19, 0, 2, 2)),
            None,
            "reaches past the right edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 0, 0, 2)),
            None,
            "no columns"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 0, 2, 0)),
            None,
            "no rows"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(18, 0, 2, 1)),
            Some(Rect::new(21, 2, 2, 1)),
            "ends exactly at the right edge"
        );
        // No overflow of u16 arithmetic.
        assert_eq!(
            visible_rect(INNER, CellRect::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX)),
            None
        );
        assert_eq!(visible_rect(INNER, CellRect::new(1, 0, u16::MAX, 1)), None);
    }
}
