//! An estimate of the memory a scene item holds, for the deck-wide budget
//! ([`super::underlay::DeckItems`]).
//!
//! The estimate is the inline size of the item plus, for every `String` / `Vec` it owns, what the
//! allocator hands out for its capacity (rounded up to 16 bytes; the allocator's own bookkeeping
//! comes on top, which is why the budget it feeds has a margin). Text shared between items
//! (`Arc<str>`) costs one pointer pair inline and nothing here: it exists once for the whole deck
//! (a name that only this item holds is counted, see `font_heap`).

use std::mem::size_of;
use std::sync::Arc;

use super::model::{
    Arrow, ArrowKind, Bullet, BulletKind, Dash, Fill, FontSpec, GeomPath, Geometry, Gradient,
    GroupItem, ImageFill, Item, Line, Paragraph, PathCmd, PicFx, PictureItem, Rgba, Run, ShapeItem,
    TabStop, TextBody,
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

// Every walk below destructures its struct with all the fields named (a field that owns nothing
// is bound to `_`) and matches its enum without a catch-all, so a `Vec` / `String` / `Arc` added
// to the model is a compile error here until it is counted (or explicitly said to cost nothing).

fn item_heap(item: &Item) -> usize {
    match item {
        Item::Shape(s) => shape_heap(s),
        Item::Picture(p) => picture_heap(p),
        Item::Group(g) => group_heap(g),
    }
}

fn shape_heap(s: &ShapeItem) -> usize {
    let ShapeItem {
        xfrm: _,
        geom,
        fill,
        line,
        text,
        text_rect: _,
        effects: _,
    } = s;
    geometry_heap(geom)
        .saturating_add(fill_heap(fill))
        .saturating_add(line.as_ref().map_or(0, line_heap))
        .saturating_add(text.as_ref().map_or(0, text_heap))
}

fn picture_heap(p: &PictureItem) -> usize {
    let PictureItem {
        xfrm: _,
        image,
        geom,
        line,
        effects: _,
    } = p;
    image_heap(image)
        .saturating_add(geometry_heap(geom))
        .saturating_add(line.as_ref().map_or(0, line_heap))
}

fn group_heap(g: &GroupItem) -> usize {
    let GroupItem {
        xfrm: _,
        child_off: _,
        child_ext: _,
        items,
    } = g;
    // The children's buffer is sized by capacity; each child adds what it owns.
    buf(items.capacity(), size_of::<Item>()).saturating_add(
        items
            .iter()
            .fold(0usize, |n, i| n.saturating_add(item_heap(i))),
    )
}

fn geometry_heap(g: &Geometry) -> usize {
    match g {
        Geometry::Paths(p) => paths_heap(p),
        Geometry::Rect | Geometry::Ellipse | Geometry::Line => 0,
    }
}

fn paths_heap(paths: &[GeomPath]) -> usize {
    // (A slice has no capacity; the readers build exact vectors for geometry.)
    buf(paths.len(), size_of::<GeomPath>()).saturating_add(paths.iter().fold(0usize, |n, p| {
        let GeomPath {
            w: _,
            h: _,
            fill_mode: _,
            stroke: _,
            cmds,
        } = p;
        n.saturating_add(buf(cmds.capacity(), size_of::<PathCmd>()))
    }))
}

fn fill_heap(f: &Fill) -> usize {
    match f {
        Fill::None | Fill::Solid(_) => 0,
        Fill::Gradient(g) => {
            let Gradient {
                kind: _,
                stops,
                fill_to_rect: _,
                rot_with_shape: _,
            } = g;
            buf(stops.capacity(), size_of::<(f64, Rgba)>())
        }
        Fill::Pattern {
            preset,
            fg: _,
            bg: _,
        } => alloc(preset.capacity()),
        Fill::Image(i) => image_heap(i),
    }
}

fn image_heap(i: &ImageFill) -> usize {
    let ImageFill {
        key,
        crop: _,
        mode: _,
        alpha: _,
        fx,
        pixelated: _,
    } = i;
    alloc(key.capacity()).saturating_add(buf(fx.capacity(), size_of::<PicFx>()))
}

fn line_heap(l: &Line) -> usize {
    let Line {
        width: _,
        fill,
        dash,
        cap: _,
        join: _,
        compound: _,
        head,
        tail,
    } = l;
    fill_heap(fill)
        .saturating_add(match dash {
            Dash::Custom(v) => buf(v.capacity(), size_of::<(f64, f64)>()),
            Dash::Solid
            | Dash::Dot
            | Dash::Dash
            | Dash::LgDash
            | Dash::DashDot
            | Dash::LgDashDot
            | Dash::LgDashDotDot
            | Dash::SysDash
            | Dash::SysDot
            | Dash::SysDashDot
            | Dash::SysDashDotDot => 0,
        })
        .saturating_add(head.as_ref().map_or(0, arrow_heap))
        .saturating_add(tail.as_ref().map_or(0, arrow_heap))
}

fn arrow_heap(a: &Arrow) -> usize {
    let Arrow { kind, w: _, len: _ } = a;
    match kind {
        ArrowKind::Custom(p) => paths_heap(p),
        ArrowKind::Triangle
        | ArrowKind::Stealth
        | ArrowKind::Diamond
        | ArrowKind::Oval
        | ArrowKind::Arrow => 0,
    }
}

fn text_heap(t: &TextBody) -> usize {
    let TextBody {
        insets: _,
        anchor: _,
        anchor_ctr: _,
        wrap: _,
        vert: _,
        autofit: _,
        rot_deg: _,
        upright: _,
        columns: _,
        paragraphs,
    } = t;
    buf(paragraphs.capacity(), size_of::<Paragraph>()).saturating_add(
        paragraphs
            .iter()
            .fold(0usize, |n, p| n.saturating_add(paragraph_heap(p))),
    )
}

fn paragraph_heap(p: &Paragraph) -> usize {
    let Paragraph {
        align: _,
        level: _,
        mar_l: _,
        indent: _,
        spc_before: _,
        spc_after: _,
        line_spacing: _,
        bullet,
        runs,
        end_size_pt: _,
        rtl: _,
        tabs,
        def_tab: _,
    } = p;
    buf(runs.capacity(), size_of::<Run>())
        .saturating_add(
            runs.iter()
                .fold(0usize, |n, r| n.saturating_add(run_heap(r))),
        )
        .saturating_add(buf(tabs.capacity(), size_of::<TabStop>()))
        .saturating_add(bullet.as_ref().map_or(0, bullet_heap))
}

fn run_heap(r: &Run) -> usize {
    let Run {
        text,
        kind: _,
        font,
        size_pt: _,
        bold: _,
        italic: _,
        underline: _,
        strike: _,
        fill,
        highlight: _,
        baseline_pct: _,
        spacing_pt: _,
        caps: _,
        lang,
    } = r;
    alloc(text.capacity())
        .saturating_add(fill_heap(fill))
        .saturating_add(font_heap(font))
        .saturating_add(name_heap(lang))
}

fn bullet_heap(b: &Bullet) -> usize {
    let Bullet {
        kind,
        font,
        color: _,
        size: _,
    } = b;
    (match kind {
        BulletKind::Char(s) => alloc(s.capacity()),
        BulletKind::AutoNum { scheme, start: _ } => alloc(scheme.capacity()),
        BulletKind::Picture(i) => image_heap(i),
    })
    .saturating_add(font.as_ref().map_or(0, font_heap))
}

/// The names of a run are shared (see `strings`: every name is interned, cut to a short length):
/// a shared name costs a pointer pair inline and nothing here, it exists once for the whole deck.
/// A name nobody else holds (`strong_count == 1`: it was not interned, or the table was cleared
/// since) is this run's own allocation and is counted, `Arc` counters included.
fn font_heap(f: &FontSpec) -> usize {
    let FontSpec {
        latin,
        east_asian,
        complex,
        symbol,
    } = f;
    [latin, east_asian, complex, symbol]
        .into_iter()
        .fold(0usize, |n, name| n.saturating_add(name_heap(name)))
}

/// The heap of one optional shared name when this is its only holder (see [`font_heap`]).
fn name_heap(name: &Option<Arc<str>>) -> usize {
    match name {
        Some(a) if Arc::strong_count(a) == 1 => alloc(ARC_HEADER.saturating_add(a.len())),
        Some(_) | None => 0,
    }
}

/// The two reference counters in front of an `Arc<str>`'s bytes.
const ARC_HEADER: usize = 2 * size_of::<usize>();

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

    #[test]
    fn a_name_nobody_else_holds_is_counted_with_its_arc_header() {
        let mut a = with_text(1, 1, 1);
        let b = item_bytes(&a);
        let own: Arc<str> = Arc::from("x".repeat(4000));
        assert_eq!(Arc::strong_count(&own), 1);
        if let Item::Shape(s) = &mut a {
            let r = &mut s.text.as_mut().unwrap().paragraphs[0].runs[0];
            r.font.latin = Some(own);
        }
        assert_eq!(item_bytes(&a), b + alloc(ARC_HEADER + 4000));
        // Each of the four typefaces and the language tag count the same way.
        if let Item::Shape(s) = &mut a {
            let r = &mut s.text.as_mut().unwrap().paragraphs[0].runs[0];
            r.font.east_asian = Some(Arc::from("e".repeat(100)));
            r.font.complex = Some(Arc::from("c".repeat(100)));
            r.font.symbol = Some(Arc::from("s".repeat(100)));
            r.lang = Some(Arc::from("l".repeat(100)));
        }
        assert_eq!(
            item_bytes(&a),
            b + alloc(ARC_HEADER + 4000) + 4 * alloc(ARC_HEADER + 100)
        );
        // A bullet's font counts too.
        let before_bullet = item_bytes(&a);
        if let Item::Shape(s) = &mut a {
            s.text.as_mut().unwrap().paragraphs[0].bullet = Some(Bullet {
                kind: BulletKind::Char(String::new()),
                font: Some(FontSpec {
                    symbol: Some(Arc::from("b".repeat(200))),
                    ..FontSpec::default()
                }),
                color: None,
                size: BulletSize::FollowText,
            });
        }
        assert_eq!(item_bytes(&a), before_bullet + alloc(ARC_HEADER + 200));
    }

    #[test]
    fn names_the_table_forgot_but_runs_still_share_cost_nothing_per_run() {
        use crate::preview::office::slide_draw::strings::{intern, MAX_NAMES};
        // The table is cleared after MAX_NAMES distinct names: names handed out before stay
        // valid and shared by the runs that hold them.
        let first = intern("first-name");
        for i in 0..MAX_NAMES + 3 {
            intern(&format!("n{i}"));
        }
        let b = item_bytes(&with_text(1, 2, 2));
        let mut a = with_text(1, 2, 2);
        if let Item::Shape(s) = &mut a {
            for r in &mut s.text.as_mut().unwrap().paragraphs[0].runs {
                r.font.latin = Some(Arc::clone(&first));
            }
        }
        // Held by two runs and by `first` itself: shared, nothing on top.
        assert_eq!(item_bytes(&a), b);
        // Once a name has a single holder it is that holder's own: counted.
        let lone: Arc<str> = Arc::from("lone-name");
        let mut c = with_text(1, 1, 1);
        let c0 = item_bytes(&c);
        if let Item::Shape(s) = &mut c {
            s.text.as_mut().unwrap().paragraphs[0].runs[0].lang = Some(lone);
        }
        assert_eq!(item_bytes(&c), c0 + alloc(ARC_HEADER + 9));
    }
}
