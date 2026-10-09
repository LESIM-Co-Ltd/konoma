//! An estimate of the memory a scene item holds, for the deck-wide budget
//! ([`super::underlay::DeckItems`]).
//!
//! The estimate is the inline size of the item plus, for every `String` / `Vec` it owns, what the
//! allocator hands out for its capacity (rounded up to 16 bytes; the allocator's own bookkeeping
//! comes on top, which is why the budget it feeds has a margin). Text shared between items
//! (`Arc<str>`) costs one pointer pair inline and nothing here: it exists once for the whole deck.

use std::mem::size_of;

use super::model::{
    Arrow, ArrowKind, Bullet, BulletKind, Dash, Fill, FontSpec, GeomPath, Geometry, GroupItem,
    ImageFill, Item, Line, Paragraph, PathCmd, PicFx, PictureItem, Rgba, Run, ShapeItem, TabStop,
    TextBody,
};

/// What the allocator really hands out for a request of `n` bytes: the size rounded up to a
/// multiple of 16 (nothing for an empty buffer).
fn alloc(n: usize) -> usize {
    n.saturating_add(15) & !15
}

/// The heap bytes of a buffer of `cap` elements of `elem` bytes.
fn buf(cap: usize, elem: usize) -> usize {
    alloc(cap.saturating_mul(elem))
}

/// Gives back the room a growing buffer kept: every `Vec` an item owns (a group's members, the
/// paragraphs and runs of its text, the commands of its paths, ...) is cut to its length. A `Vec`
/// grown by pushes holds at least four elements, and a paragraph or a run is several hundred
/// bytes: a shape with one paragraph of one run kept about 2 KB it never used. The readers call
/// this on a finished list of items, once.
pub fn compact(items: &mut Vec<Item>) {
    items.shrink_to_fit();
    items.iter_mut().for_each(compact_item);
}

fn compact_item(item: &mut Item) {
    match item {
        Item::Shape(s) => {
            compact_geometry(&mut s.geom);
            if let Some(t) = s.text.as_mut() {
                compact_text(t);
            }
            if let Some(l) = s.line.as_mut() {
                compact_line(l);
            }
        }
        Item::Picture(p) => {
            compact_geometry(&mut p.geom);
            p.image.fx.shrink_to_fit();
            if let Some(l) = p.line.as_mut() {
                compact_line(l);
            }
        }
        Item::Group(g) => compact(&mut g.items),
    }
}

fn compact_geometry(g: &mut Geometry) {
    if let Geometry::Paths(paths) = g {
        paths.shrink_to_fit();
        paths.iter_mut().for_each(|p| p.cmds.shrink_to_fit());
    }
}

fn compact_line(l: &mut Line) {
    for a in [l.head.as_mut(), l.tail.as_mut()].into_iter().flatten() {
        if let ArrowKind::Custom(paths) = &mut a.kind {
            paths.shrink_to_fit();
            paths.iter_mut().for_each(|p| p.cmds.shrink_to_fit());
        }
    }
}

fn compact_text(t: &mut TextBody) {
    t.paragraphs.shrink_to_fit();
    for p in &mut t.paragraphs {
        p.runs.shrink_to_fit();
        p.tabs.shrink_to_fit();
        p.runs.iter_mut().for_each(|r| r.text.shrink_to_fit());
    }
}

/// Whether any buffer of the list (or of an item in it) holds room it does not use: what
/// [`compact`] removes. For the tests of the readers.
#[cfg(test)]
#[allow(clippy::ptr_arg)] // the capacity of the list itself is the point
pub(crate) fn has_slack(items: &Vec<Item>) -> bool {
    fn paths(p: &[GeomPath]) -> bool {
        p.iter().any(|p| p.cmds.capacity() != p.cmds.len())
    }
    items.capacity() != items.len()
        || items.iter().any(|i| match i {
            Item::Shape(s) => {
                matches!(&s.geom, Geometry::Paths(p) if paths(p))
                    || s.text.as_ref().is_some_and(|t| {
                        t.paragraphs.capacity() != t.paragraphs.len()
                            || t.paragraphs.iter().any(|p| {
                                p.runs.capacity() != p.runs.len()
                                    || p.tabs.capacity() != p.tabs.len()
                                    || p.runs.iter().any(|r| r.text.capacity() != r.text.len())
                            })
                    })
            }
            Item::Picture(p) => p.image.fx.capacity() != p.image.fx.len(),
            Item::Group(g) => has_slack(&g.items),
        })
}

/// The bytes one item holds: its own size and what it owns on the heap, groups included.
pub fn item_bytes(item: &Item) -> usize {
    size_of::<Item>().saturating_add(item_heap(item))
}

/// The bytes a list of items holds (each item's own size and what it owns; the slack of the
/// list's growing buffer is not counted).
pub fn items_bytes(items: &[Item]) -> usize {
    items
        .iter()
        .fold(0usize, |n, i| n.saturating_add(item_bytes(i)))
}

fn item_heap(item: &Item) -> usize {
    match item {
        Item::Shape(s) => shape_heap(s),
        Item::Picture(p) => picture_heap(p),
        Item::Group(g) => group_heap(g),
    }
}

fn shape_heap(s: &ShapeItem) -> usize {
    geometry_heap(&s.geom)
        .saturating_add(fill_heap(&s.fill))
        .saturating_add(s.line.as_ref().map_or(0, line_heap))
        .saturating_add(s.text.as_ref().map_or(0, text_heap))
}

fn picture_heap(p: &PictureItem) -> usize {
    image_heap(&p.image)
        .saturating_add(geometry_heap(&p.geom))
        .saturating_add(p.line.as_ref().map_or(0, line_heap))
}

fn group_heap(g: &GroupItem) -> usize {
    // The children's buffer is sized by capacity; each child adds what it owns.
    buf(g.items.capacity(), size_of::<Item>()).saturating_add(
        g.items
            .iter()
            .fold(0usize, |n, i| n.saturating_add(item_heap(i))),
    )
}

fn geometry_heap(g: &Geometry) -> usize {
    match g {
        Geometry::Paths(p) => paths_heap(p),
        _ => 0,
    }
}

fn paths_heap(paths: &[GeomPath]) -> usize {
    // (A slice has no capacity; the readers build exact vectors for geometry.)
    buf(paths.len(), size_of::<GeomPath>()).saturating_add(paths.iter().fold(0usize, |n, p| {
        n.saturating_add(buf(p.cmds.capacity(), size_of::<PathCmd>()))
    }))
}

fn fill_heap(f: &Fill) -> usize {
    match f {
        Fill::None | Fill::Solid(_) => 0,
        Fill::Gradient(g) => buf(g.stops.capacity(), size_of::<(f64, Rgba)>()),
        Fill::Pattern { preset, .. } => alloc(preset.capacity()),
        Fill::Image(i) => image_heap(i),
    }
}

fn image_heap(i: &ImageFill) -> usize {
    alloc(i.key.capacity()).saturating_add(buf(i.fx.capacity(), size_of::<PicFx>()))
}

fn line_heap(l: &Line) -> usize {
    fill_heap(&l.fill)
        .saturating_add(match &l.dash {
            Dash::Custom(v) => buf(v.capacity(), size_of::<(f64, f64)>()),
            _ => 0,
        })
        .saturating_add(l.head.as_ref().map_or(0, arrow_heap))
        .saturating_add(l.tail.as_ref().map_or(0, arrow_heap))
}

fn arrow_heap(a: &Arrow) -> usize {
    match &a.kind {
        ArrowKind::Custom(p) => paths_heap(p),
        _ => 0,
    }
}

fn text_heap(t: &TextBody) -> usize {
    buf(t.paragraphs.capacity(), size_of::<Paragraph>()).saturating_add(
        t.paragraphs
            .iter()
            .fold(0usize, |n, p| n.saturating_add(paragraph_heap(p))),
    )
}

fn paragraph_heap(p: &Paragraph) -> usize {
    buf(p.runs.capacity(), size_of::<Run>())
        .saturating_add(
            p.runs
                .iter()
                .fold(0usize, |n, r| n.saturating_add(run_heap(r))),
        )
        .saturating_add(buf(p.tabs.capacity(), size_of::<TabStop>()))
        .saturating_add(p.bullet.as_ref().map_or(0, bullet_heap))
}

fn run_heap(r: &Run) -> usize {
    alloc(r.text.capacity())
        .saturating_add(fill_heap(&r.fill))
        .saturating_add(font_heap(&r.font))
}

fn bullet_heap(b: &Bullet) -> usize {
    (match &b.kind {
        BulletKind::Char(s) => alloc(s.capacity()),
        BulletKind::AutoNum { scheme, .. } => alloc(scheme.capacity()),
        BulletKind::Picture(i) => image_heap(i),
    })
    .saturating_add(b.font.as_ref().map_or(0, font_heap))
}

/// The names of a run are shared (see `strings`): each costs a pointer pair inline and nothing
/// here.
fn font_heap(_: &FontSpec) -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::office::slide_draw::model::{
        ArrowSize, BulletSize, GradKind, Gradient, PathFill, Xfrm,
    };
    use std::sync::Arc;

    fn plain() -> ShapeItem {
        ShapeItem::new(Xfrm::rect(0.0, 0.0, 1.0, 1.0), Geometry::Rect)
    }

    fn with_text(paragraphs: usize, runs: usize, cap: usize) -> Item {
        let mut s = plain();
        let mut body = TextBody {
            paragraphs: Vec::with_capacity(cap),
            ..TextBody::default()
        };
        for _ in 0..paragraphs {
            let mut p = Paragraph {
                runs: Vec::with_capacity(cap),
                ..Paragraph::default()
            };
            for _ in 0..runs {
                p.runs.push(Run::text("hello", 12.0));
            }
            body.paragraphs.push(p);
        }
        s.text = Some(body);
        Item::Shape(s)
    }

    #[test]
    fn alloc_rounds_up_to_sixteen_and_an_empty_buffer_is_nothing() {
        assert_eq!(alloc(0), 0);
        assert_eq!(alloc(1), 16);
        assert_eq!(alloc(16), 16);
        assert_eq!(alloc(17), 32);
        // (No overflow at the far end.)
        assert!(alloc(usize::MAX) >= alloc(usize::MAX - 1));
        assert_eq!(buf(0, 56), 0);
        assert_eq!(buf(3, 56), 176);
        assert!(buf(usize::MAX, 56) >= alloc(usize::MAX - 1));
    }

    #[test]
    fn a_bare_shape_is_its_inline_size() {
        assert_eq!(item_bytes(&Item::Shape(plain())), size_of::<Item>());
        assert_eq!(items_bytes(&[]), 0);
        let two = [Item::Shape(plain()), Item::Shape(plain())];
        assert_eq!(items_bytes(&two), 2 * size_of::<Item>());
    }

    #[test]
    fn text_adds_its_paragraphs_runs_and_strings() {
        let bare = item_bytes(&Item::Shape(plain()));
        let one = item_bytes(&with_text(1, 1, 1));
        let three = item_bytes(&with_text(1, 3, 3));
        let two_paras = item_bytes(&with_text(2, 1, 2));
        assert!(one > bare + size_of::<Paragraph>() + size_of::<Run>());
        assert!(three > one + 2 * size_of::<Run>());
        assert!(two_paras > one + size_of::<Paragraph>());
        // A longer text costs more, by its length (rounded up to the allocator's step).
        let mut long = with_text(1, 1, 1);
        if let Item::Shape(s) = &mut long {
            s.text.as_mut().unwrap().paragraphs[0].runs[0].text = "x".repeat(1000);
        }
        assert!(item_bytes(&long) >= one + 1000 - 16);
    }

    #[test]
    fn unused_capacity_is_counted_and_compact_gives_it_back() {
        let mut items = vec![with_text(1, 1, 8), with_text(2, 2, 8)];
        items.reserve(30);
        assert!(has_slack(&items));
        let before = items_bytes(&items);
        compact(&mut items);
        let after = items_bytes(&items);
        assert!(after + 8 * size_of::<Run>() < before, "{before} -> {after}");
        assert_eq!(items.capacity(), items.len());
        for it in &items {
            let Item::Shape(s) = it else { panic!() };
            let t = s.text.as_ref().unwrap();
            assert_eq!(t.paragraphs.capacity(), t.paragraphs.len());
            for p in &t.paragraphs {
                assert_eq!(p.runs.capacity(), p.runs.len());
            }
        }
        assert!(!has_slack(&items));
        // Done once, it is done: nothing more to give back.
        compact(&mut items);
        assert_eq!(items_bytes(&items), after);
    }

    #[test]
    fn compact_reaches_groups_paths_lines_and_pictures() {
        let mut path = GeomPath {
            cmds: Vec::with_capacity(64),
            ..GeomPath::default()
        };
        path.cmds.push(PathCmd::Close);
        let mut shape = plain();
        shape.geom = Geometry::Paths({
            let mut v = Vec::with_capacity(8);
            v.push(path.clone());
            v
        });
        let mut line = Line::solid(1.0, Rgba::BLACK);
        line.head = Some(Arrow {
            kind: ArrowKind::Custom({
                let mut v = Vec::with_capacity(8);
                v.push(path);
                v
            }),
            w: ArrowSize::Med,
            len: ArrowSize::Med,
        });
        shape.line = Some(line);
        let mut pic = PictureItem::new(Xfrm::rect(0.0, 0.0, 1.0, 1.0), "key");
        pic.image.fx = Vec::with_capacity(8);
        let mut members = Vec::with_capacity(16);
        members.push(Item::Shape(shape));
        members.push(Item::Picture(pic));
        let group = Item::Group(GroupItem {
            xfrm: Xfrm::rect(0.0, 0.0, 1.0, 1.0),
            child_off: (0.0, 0.0),
            child_ext: (1.0, 1.0),
            items: members,
        });
        let mut items = vec![group];
        let before = items_bytes(&items);
        compact(&mut items);
        let after = items_bytes(&items);
        assert!(after < before, "{before} -> {after}");
        let Item::Group(g) = &items[0] else { panic!() };
        assert_eq!(g.items.capacity(), g.items.len());
        let Item::Shape(s) = &g.items[0] else {
            panic!()
        };
        let Geometry::Paths(p) = &s.geom else {
            panic!()
        };
        assert_eq!(p.capacity(), p.len());
        assert_eq!(p[0].cmds.capacity(), p[0].cmds.len());
        let head = s.line.as_ref().and_then(|l| l.head.as_ref()).unwrap();
        let ArrowKind::Custom(h) = &head.kind else {
            panic!()
        };
        assert_eq!(h.capacity(), h.len());
        assert_eq!(h[0].cmds.capacity(), h[0].cmds.len());
        let Item::Picture(pi) = &g.items[1] else {
            panic!()
        };
        assert_eq!(pi.image.fx.capacity(), 0);
    }

    #[test]
    fn fills_strings_and_bullets_are_counted() {
        let mut s = plain();
        let base = item_bytes(&Item::Shape(s.clone()));
        s.fill = Fill::Pattern {
            preset: "p".repeat(100),
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
        };
        assert!(item_bytes(&Item::Shape(s.clone())) >= base + 100);
        s.fill = Fill::Gradient(Gradient {
            kind: GradKind::Radial,
            stops: vec![(0.0, Rgba::BLACK), (1.0, Rgba::WHITE)],
            fill_to_rect: (0.0, 0.0, 0.0, 0.0),
            rot_with_shape: true,
        });
        assert!(item_bytes(&Item::Shape(s.clone())) >= base + 2 * size_of::<(f64, Rgba)>());
        s.fill = Fill::Image(ImageFill::stretch("k".repeat(200)));
        assert!(item_bytes(&Item::Shape(s.clone())) >= base + 200);
        // A bullet's text and a picture bullet's key.
        let mut with_bullet = with_text(1, 1, 1);
        let plain_bytes = item_bytes(&with_bullet);
        for (kind, at_least) in [
            (
                BulletKind::Picture(ImageFill::stretch("b".repeat(300))),
                300,
            ),
            (BulletKind::Char("x".repeat(40)), 40),
            (
                BulletKind::AutoNum {
                    scheme: "s".repeat(40),
                    start: 1,
                },
                40,
            ),
        ] {
            if let Item::Shape(sh) = &mut with_bullet {
                sh.text.as_mut().unwrap().paragraphs[0].bullet = Some(Bullet {
                    kind,
                    font: None,
                    color: None,
                    size: BulletSize::FollowText,
                });
            }
            assert!(item_bytes(&with_bullet) >= plain_bytes + at_least);
        }
    }

    #[test]
    fn a_dash_pattern_and_path_commands_are_counted() {
        let mut s = plain();
        let base = item_bytes(&Item::Shape(s.clone()));
        let mut line = Line::solid(1.0, Rgba::BLACK);
        line.dash = Dash::Custom(vec![(1.0, 2.0); 10]);
        s.line = Some(line);
        assert!(item_bytes(&Item::Shape(s.clone())) >= base + 10 * size_of::<(f64, f64)>());
        let p = GeomPath {
            fill_mode: PathFill::Lighten,
            cmds: vec![PathCmd::Close; 20],
            ..GeomPath::default()
        };
        s.geom = Geometry::Paths(vec![p]);
        assert!(item_bytes(&Item::Shape(s)) >= base + 20 * size_of::<PathCmd>());
    }

    #[test]
    fn shared_names_cost_a_run_nothing_on_the_heap() {
        let name: Arc<str> = Arc::from("Calibri");
        let mut a = with_text(1, 1, 1);
        let b = item_bytes(&a);
        if let Item::Shape(s) = &mut a {
            let r = &mut s.text.as_mut().unwrap().paragraphs[0].runs[0];
            r.font.latin = Some(Arc::clone(&name));
            r.font.east_asian = Some(Arc::clone(&name));
            r.lang = Some(Arc::clone(&name));
        }
        assert_eq!(item_bytes(&a), b);
    }

    #[test]
    fn a_group_counts_its_members_and_their_buffer() {
        let leaf = Item::Shape(plain());
        let g = Item::Group(GroupItem {
            xfrm: Xfrm::rect(0.0, 0.0, 1.0, 1.0),
            child_off: (0.0, 0.0),
            child_ext: (1.0, 1.0),
            items: vec![leaf.clone(), leaf, with_text(1, 1, 1)],
        });
        let inner = item_bytes(&with_text(1, 1, 1));
        assert!(item_bytes(&g) >= size_of::<Item>() + 2 * size_of::<Item>() + inner);
    }
}
