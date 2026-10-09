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
/// each. This is the guard on the work of building scenes; the memory they hold is bounded by
/// [`MAX_DECK_BYTES`], which is the tighter limit for anything but small items. A real deck of
/// 1,000 slides with 50 shapes each is 50,000. Slides after a limit are drawn partially or not at
/// all and marked truncated.
pub const MAX_DECK_ITEMS: usize = 100_000;

/// The estimated bytes ([`super::footprint`]) the scenes of a deck may hold in all: the shared
/// master / layout lists once and every slide's own items, copies of a master's shapes excluded
/// (see [`MAX_COPY_BYTES`]). Together with the copies the scenes of a deck stay under
/// `MAX_DECK_BYTES + MAX_COPY_BYTES` = 256 MiB, plus at most what the slide that crosses the line
/// holds (it is built under an item cap taken from the bytes left, [`BYTES_PER_ITEM`] an item).
/// Measured on the hostile decks of the second review: 100,000 items took 1.2-1.6 GB when the
/// budget was a count of items only.
pub const MAX_DECK_BYTES: usize = 192 << 20;

/// The estimated bytes the per-slide copies of a master's or a layout's shapes that show the
/// slide's own number or name may hold in all. They have a budget of their own so that a master
/// full of such shapes cannot use up what the slides' own content needs: the slides keep their
/// content, the later copies are left out (and the document is marked truncated).
pub const MAX_COPY_BYTES: usize = 64 << 20;

/// What an item is assumed to hold when the bytes left are turned into a number of items to build
/// (the cap a slide is built under). The exact bytes are charged afterwards, so a slide of bigger
/// items overshoots by at most the items it was allowed; a slide of smaller ones leaves the rest
/// for the slides after it.
pub const BYTES_PER_ITEM: usize = 2048;

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
            let mut run = std::mem::take(&mut self.run);
            // (Held by every slide that shows the part: no room kept for growth.)
            run.shrink_to_fit();
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

/// What is still available to the slides of a deck: items and estimated bytes, in two pools --
/// one for the shared drawings and the slides' own items, one for the copies of a master's shapes
/// that show the slide's number or name.
#[derive(Debug, Clone, Copy)]
pub struct DeckItems {
    used: usize,
    limit: usize,
    bytes: usize,
    bytes_limit: usize,
    copy_used: usize,
    copy_limit: usize,
    copy_bytes: usize,
    copy_bytes_limit: usize,
}

impl DeckItems {
    /// A budget of `limit` items and [`MAX_DECK_BYTES`] bytes; the copies get a quarter of the
    /// items and [`MAX_COPY_BYTES`].
    #[cfg(test)]
    pub fn new(limit: usize) -> DeckItems {
        DeckItems::with_bytes(limit, MAX_DECK_BYTES, MAX_COPY_BYTES)
    }

    /// A budget of `limit` items and `bytes` bytes, and a pool for copies of `copy_bytes` bytes
    /// and a quarter of the items.
    pub fn with_bytes(limit: usize, bytes: usize, copy_bytes: usize) -> DeckItems {
        DeckItems {
            used: 0,
            limit,
            bytes: 0,
            bytes_limit: bytes,
            copy_used: 0,
            copy_limit: limit / 4,
            copy_bytes: 0,
            copy_bytes_limit: copy_bytes,
        }
    }

    /// How many more items may be built: what is left of the item count, and of the bytes at
    /// [`BYTES_PER_ITEM`] an item, whichever is less.
    pub fn left(&self) -> usize {
        self.limit
            .saturating_sub(self.used)
            .min(self.bytes_limit.saturating_sub(self.bytes) / BYTES_PER_ITEM)
    }

    /// [`Self::left`] for the copies of a master's shapes.
    pub fn copies_left(&self) -> usize {
        self.copy_limit
            .saturating_sub(self.copy_used)
            .min(self.copy_bytes_limit.saturating_sub(self.copy_bytes) / BYTES_PER_ITEM)
    }

    /// The estimated bytes still available to the copies of a master's shapes.
    pub fn copies_bytes_left(&self) -> usize {
        self.copy_bytes_limit.saturating_sub(self.copy_bytes)
    }

    /// Spends `n` items that hold `bytes`.
    pub fn spend(&mut self, n: usize, bytes: usize) {
        self.used = self.used.saturating_add(n);
        self.bytes = self.bytes.saturating_add(bytes);
    }

    /// [`Self::spend`] from the pool of the copies.
    pub fn spend_copies(&mut self, n: usize, bytes: usize) {
        self.copy_used = self.copy_used.saturating_add(n);
        self.copy_bytes = self.copy_bytes.saturating_add(bytes);
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
        d.spend(4, 0);
        assert_eq!(d.left(), 6);
        d.spend(100, 0);
        assert_eq!(d.left(), 0);
        d.spend(usize::MAX, usize::MAX);
        assert_eq!(d.left(), 0);
    }

    #[test]
    fn the_bytes_left_limit_the_items_at_a_fixed_size_each() {
        let mut d = DeckItems::with_bytes(1_000_000, 10 * BYTES_PER_ITEM, 0);
        assert_eq!(d.left(), 10);
        // Three items that hold as much as eight assumed ones.
        d.spend(3, 8 * BYTES_PER_ITEM);
        assert_eq!(d.left(), 2);
        d.spend(1, 100 * BYTES_PER_ITEM);
        assert_eq!(d.left(), 0);
        // (Small items leave room: the count is not what runs out.)
        let mut d = DeckItems::with_bytes(1_000_000, 10 * BYTES_PER_ITEM, 0);
        d.spend(50, 10);
        assert_eq!(d.left(), 9);
    }

    #[test]
    fn the_copies_have_a_pool_of_their_own() {
        let mut d = DeckItems::with_bytes(100, 100 * BYTES_PER_ITEM, 100 * BYTES_PER_ITEM);
        assert_eq!((d.left(), d.copies_left()), (100, 25));
        d.spend_copies(25, 0);
        // The copies ran out; what the slides' own items may use is untouched.
        assert_eq!((d.left(), d.copies_left()), (100, 0));
        d.spend(100, 0);
        assert_eq!(d.left(), 0);
        let mut d = DeckItems::with_bytes(1_000, 1_000 * BYTES_PER_ITEM, 3 * BYTES_PER_ITEM);
        assert_eq!(d.copies_left(), 3);
        d.spend_copies(1, 2 * BYTES_PER_ITEM);
        assert_eq!(d.copies_left(), 1);
        d.spend_copies(usize::MAX, usize::MAX);
        assert_eq!(d.copies_left(), 0);
    }
}
