//! Metafiles used many times in one slide are converted once, and a conversion stops when it is
//! told it is not wanted.

use std::cell::Cell;
use std::sync::Arc;

use super::tests::{rgb, Emf};
use super::{to_svg, to_svg_cancellable, CONVERSIONS};
use crate::preview::office::slide_draw::tests::{e, scene};
use crate::preview::office::slide_draw::{render_svg, Item, PictureItem, Xfrm};

/// An EMF of `n` small rectangles.
fn emf_of(n: usize) -> Vec<u8> {
    let mut m = Emf::new(200, 200);
    m.solid(1, rgb(200, 30, 30)).select(1);
    for i in 0..n {
        let v = (i % 180) as i32;
        m.rect(v, v, v + 10, v + 10);
    }
    m.finish()
}

fn conversions() -> usize {
    CONVERSIONS.with(|c| c.get())
}

#[test]
fn a_metafile_used_many_times_is_converted_once_per_render() {
    let bytes = Arc::new(emf_of(5));
    let media = {
        let b = bytes.clone();
        move |_: &str| Some(b.clone())
    };
    let items: Vec<Item> = (0..50)
        .map(|i| {
            Item::Picture(PictureItem::new(
                Xfrm::rect(
                    e((i % 10) as f64 * 90.0),
                    e((i / 10) as f64 * 90.0),
                    e(80.0),
                    e(80.0),
                ),
                "m".to_string(),
            ))
        })
        .collect();
    let before = conversions();
    let r = render_svg(&scene(items), &media);
    assert_eq!(
        conversions() - before,
        1,
        "one conversion, not 50 (nor 100)"
    );
    assert_eq!(r.svg.matches("<image ").count(), 1);
    assert_eq!(r.svg.matches("<use ").count(), 50);
}

#[test]
fn a_metafile_tiled_and_drawn_as_a_picture_is_still_converted_once() {
    use crate::preview::office::slide_draw::{
        Fill, Geometry, ImageFill, ImageMode, RectAlign, ShapeItem, TileFlip,
    };
    let bytes = Arc::new(emf_of(5));
    let media = {
        let b = bytes.clone();
        move |_: &str| Some(b.clone())
    };
    let mut fill = ImageFill::stretch("m".to_string());
    fill.mode = ImageMode::Tile {
        sx: 1.0,
        sy: 1.0,
        tx: 0.0,
        ty: 0.0,
        align: RectAlign::TopLeft,
        flip: TileFlip::None,
    };
    let mut shape = ShapeItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(400.0), e(300.0)),
        Geometry::Rect,
    );
    shape.fill = Fill::Image(fill);
    let pic = PictureItem::new(
        Xfrm::rect(e(0.0), e(310.0), e(100.0), e(100.0)),
        "m".to_string(),
    );
    let before = conversions();
    let r = render_svg(&scene(vec![Item::Shape(shape), Item::Picture(pic)]), &media);
    // the tile asks for the natural size and for the picture; the picture asks again
    assert_eq!(conversions() - before, 1);
    assert_eq!(r.svg.matches("<image ").count(), 1);
}

#[test]
fn a_cancelled_conversion_gives_up() {
    let big = emf_of(5000);
    assert!(to_svg(&big).is_some());
    assert!(to_svg_cancellable(&big, &|| false).is_some());
    assert!(to_svg_cancellable(&big, &|| true).is_none());
    // polled every so many records, not only at the start
    let polls = Cell::new(0usize);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() >= 3
    };
    assert!(to_svg_cancellable(&big, &cancel).is_none());
    assert_eq!(polls.get(), 3);
    // a file shorter than one polling interval is never interrupted
    let small = emf_of(10);
    assert!(to_svg_cancellable(&small, &|| true).is_some());
}

#[test]
fn a_metafile_of_a_hundred_thousand_shapes_is_cut_at_its_budgets() {
    // 100 000 rectangles: past the markup budget (3 MiB) long before the end of the file
    let big = emf_of(100_000);
    let m = to_svg(&big).expect("converts");
    assert!(m.truncated);
    assert!(
        m.svg.len() <= super::gdi::MAX_MARKUP_BYTES + 100_000,
        "{}",
        m.svg.len()
    );
    let drawn = m.svg.matches("<path ").count() as u64;
    assert!(drawn <= super::gdi::MAX_DRAWN + 1, "{drawn}");
    // a file well inside both is whole
    let ok = to_svg(&emf_of(2000)).unwrap();
    assert!(!ok.truncated);
    assert_eq!(ok.svg.matches("<path ").count(), 2000);
}

#[test]
fn a_wmf_polypolygon_is_bounded_in_its_total_points_like_an_emf_one() {
    use super::tests::Wmf;
    let poly = |polys: usize| {
        let mut v = vec![polys as i32];
        v.extend(std::iter::repeat_n(1000, polys));
        for i in 0..polys * 1000 {
            v.push((i % 100) as i32);
            v.push((i * 7 % 100) as i32);
        }
        let mut m = Wmf::new(100, 100);
        m.brush(0, 0x0000ff, 0).select(0);
        m.r(0x0538, &v);
        m.finish()
    };
    // 100 000 points are drawn, 300 000 are refused (the record is dropped)
    let ok = to_svg(&poly(100)).expect("converts");
    assert!(
        ok.svg.contains("<path "),
        "{}",
        &ok.svg[..ok.svg.len().min(300)]
    );
    let too_many = to_svg(&poly(300)).expect("converts");
    assert!(!too_many.svg.contains("<path "));
}
