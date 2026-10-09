//! What the readers share between the slides of one deck: the drawing of a master or a layout is
//! built once and every slide that shows it holds the same list ([`SlideScene::underlay`]), and
//! the number of scene items over the whole deck is budgeted ([`DeckItems`]).
//!
//! Without this a deck of 1,000 slides over a master of 4,500 shapes held 4.5 million items
//! (9 GB) although the master was one drawing.

use std::sync::Arc;

use super::Item;

/// Most scene items (shapes, pictures and group members, counted as the readers count them) a
/// deck keeps in all its slides: the shared master / layout lists once, every slide's own items
/// each. Measured: one item is about 2 KB of scene (a text shape with a few runs), so the limit is
/// about 200 MB; a real deck of 1,000 slides with 50 shapes each is 50,000. Slides after the limit
/// are drawn partially or not at all and marked truncated.
pub const MAX_DECK_ITEMS: usize = 100_000;

/// One stretch of a master's or a layout's drawing, in paint order.
#[derive(Debug, Clone)]
pub enum Seg {
    /// Drawn the same on every slide: shared.
    Shared(Arc<Vec<Item>>),
    /// The top-level shape of this index shows something of the slide it is on (its number or
    /// name): the reader builds it again for every slide, between the shared stretches.
    Own(usize),
}

/// A master's or a layout's drawing, as the first slide that showed it built it.
#[derive(Debug, Clone, Default)]
pub struct PartDraw {
    /// The stretches, back to front.
    pub segs: Vec<Seg>,
    /// How many shapes the reader counted building the shared stretches (what a slide that shows
    /// them has already used of its own item budget).
    pub count: usize,
    /// How many text characters they hold (the same for the slide's text budget).
    pub chars: usize,
    /// A budget cut the drawing short.
    pub truncated: bool,
}

/// Collects the top-level shapes of a part into stretches while the reader builds them.
#[derive(Debug, Default)]
pub struct PartBuilder {
    out: PartDraw,
    run: Vec<Item>,
}

impl PartBuilder {
    /// A shape that is the same on every slide: `items` are what it was drawn as, `count` / `chars`
    /// what the reader counted for it.
    pub fn shared(&mut self, items: Vec<Item>, count: usize, chars: usize) {
        self.run.extend(items);
        self.out.count += count;
        self.out.chars += chars;
    }

    /// The shape at `index` belongs to the slide it was built for: the shared stretch before it
    /// ends here and the shape gets a place of its own.
    pub fn own(&mut self, index: usize) {
        self.end_run();
        self.out.segs.push(Seg::Own(index));
    }

    fn end_run(&mut self) {
        if !self.run.is_empty() {
            let run = std::mem::take(&mut self.run);
            self.out.segs.push(Seg::Shared(Arc::new(run)));
        }
    }

    /// The finished drawing; `truncated` says a budget cut it.
    pub fn finish(mut self, truncated: bool) -> PartDraw {
        self.end_run();
        self.out.truncated = truncated;
        self.out
    }
}

/// The items still available to the slides of a deck.
#[derive(Debug, Clone, Copy)]
pub struct DeckItems {
    used: usize,
    limit: usize,
}

impl DeckItems {
    /// A budget of `limit` items.
    pub fn new(limit: usize) -> DeckItems {
        DeckItems { used: 0, limit }
    }

    /// How many more items may be built.
    pub fn left(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }

    /// Spends `n` items.
    pub fn spend(&mut self, n: usize) {
        self.used = self.used.saturating_add(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item::Shape(crate::preview::office::slide_draw::ShapeItem::new(
            crate::preview::office::slide_draw::Xfrm::rect(0.0, 0.0, 1.0, 1.0),
            crate::preview::office::slide_draw::Geometry::Rect,
        ))
    }

    #[test]
    fn a_part_builder_keeps_shared_stretches_and_the_places_of_own_shapes() {
        let mut b = PartBuilder::default();
        b.shared(vec![item(), item()], 2, 5);
        b.shared(vec![item()], 1, 3);
        b.own(2);
        b.own(3);
        b.shared(vec![item()], 1, 0);
        let d = b.finish(true);
        assert_eq!((d.count, d.chars, d.truncated), (4, 8, true));
        assert_eq!(d.segs.len(), 4);
        assert!(matches!(&d.segs[0], Seg::Shared(l) if l.len() == 3));
        assert!(matches!(d.segs[1], Seg::Own(2)));
        assert!(matches!(d.segs[2], Seg::Own(3)));
        assert!(matches!(&d.segs[3], Seg::Shared(l) if l.len() == 1));
    }

    #[test]
    fn a_shape_with_nothing_drawn_leaves_no_empty_stretch() {
        let mut b = PartBuilder::default();
        b.shared(Vec::new(), 1, 0);
        b.own(0);
        b.shared(Vec::new(), 1, 0);
        let d = b.finish(false);
        assert_eq!(d.segs.len(), 1);
        assert!(matches!(d.segs[0], Seg::Own(0)));
        assert_eq!(d.count, 2);
        assert!(!d.truncated);
        assert!(PartBuilder::default().finish(false).segs.is_empty());
    }

    #[test]
    fn deck_items_run_out_and_never_underflow() {
        let mut d = DeckItems::new(10);
        assert_eq!(d.left(), 10);
        d.spend(4);
        assert_eq!(d.left(), 6);
        d.spend(100);
        assert_eq!(d.left(), 0);
        d.spend(usize::MAX);
        assert_eq!(d.left(), 0);
    }
}
