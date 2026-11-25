use crate::markdown::BlockAlign;
use crate::picture_overlay::{
    draw_inline_pictures, InlineOverlayValues, InlinePicturePlacement, PicturePaintKind,
    PicturePaintRequest,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

fn values() -> InlineOverlayValues<'static> {
    InlineOverlayValues {
        inner: Rect::new(2, 1, 16, 7),
        scroll: 0,
        gutter: 1,
        focused_ordinal: None,
        zoom: 1.0,
        image_align: BlockAlign::Center,
        focus_title: " focus ",
    }
}

fn placement() -> InlinePicturePlacement<'static> {
    InlinePicturePlacement {
        url: "mermaid-fence://one",
        visual_row: 2,
        col: 4,
        cols: 4,
        rows: 3,
        fence_ord: Some(3),
    }
}

fn buffer() -> Buffer {
    let mut b = Buffer::empty(Rect::new(0, 0, 20, 10));
    for c in &mut b.content {
        c.set_symbol("x").set_style(
            Style::new()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );
    }
    b
}

fn cells_equal(actual: &Buffer, expected: &Buffer) {
    let rect = Rect::new(0, 0, 20, 10);
    assert_eq!(actual.area, rect);
    assert_eq!(expected.area, rect);
    let mut count = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let (a, b) = (&actual[(x, y)], &expected[(x, y)]);
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (b.symbol(), b.fg, b.bg, b.modifier),
                "{x},{y}"
            );
            count += 1;
            nonempty += usize::from(a.symbol() != " ");
        }
    }
    assert_eq!(count, 200);
    assert!(nonempty > 0);
}

fn draw(
    v: InlineOverlayValues<'_>,
    p: &[InlinePicturePlacement<'_>],
    ready: bool,
) -> (Buffer, Option<u64>, Vec<PicturePaintRequest>) {
    let mut b = buffer();
    let mut requests = Vec::new();
    let sig = draw_inline_pictures(&mut b, p.iter().copied(), v, |_, r, _| {
        requests.push(r);
        ready
    });
    (b, sig, requests)
}

#[test]
fn inline_overlay_cells_and_requests_match_both_adapters() {
    let mut v = values();
    v.scroll = 3;
    let (b, _, requests) = draw(v, &[placement()], false);
    assert_eq!(
        requests,
        vec![
            PicturePaintRequest {
                target: Rect::new(7, 1, 4, 2),
                cols: 4,
                rows: 3,
                row_offset: 1,
                visible_rows: 2,
                kind: PicturePaintKind::Density
            },
            PicturePaintRequest {
                target: Rect::new(7, 1, 4, 2),
                cols: 4,
                rows: 3,
                row_offset: 1,
                visible_rows: 2,
                kind: PicturePaintKind::Normal
            }
        ]
    );
    let mut expected = buffer();
    // Frozen pre-extraction konoma cells: symbols and backgrounds survive the reset.
    for y in 1..3 {
        for x in 7..11 {
            expected[(x, y)].fg = Color::Reset;
            expected[(x, y)].modifier = Modifier::empty();
        }
    }
    cells_equal(&b, &expected);
}

#[test]
fn focused_fence_uses_source_ordinal_and_full_visibility() {
    let mut v = values();
    v.focused_ordinal = Some(0);
    v.zoom = 2.0;
    let (unfocused, _, r) = draw(v, &[placement()], false);
    assert!(!r.iter().any(|r| r.kind == PicturePaintKind::Zoom));
    v.focused_ordinal = Some(3);
    let (focused, _, r) = draw(v, &[placement()], false);
    assert_eq!(
        r.iter().map(|r| r.kind).collect::<Vec<_>>(),
        vec![
            PicturePaintKind::Density,
            PicturePaintKind::Zoom,
            PicturePaintKind::UnzoomedFallback,
            PicturePaintKind::Normal
        ]
    );
    assert_eq!(focused[(5, 2)].symbol(), "┌");
    assert_eq!(focused[(13, 6)].symbol(), "┘");
    v.scroll = 3;
    let (partial, _, r) = draw(v, &[placement()], false);
    assert!(!r.iter().any(|r| r.kind == PicturePaintKind::Zoom));
    let mut expected = buffer();
    for y in 1..3 {
        for x in 7..11 {
            expected[(x, y)].fg = Color::Reset;
            expected[(x, y)].modifier = Modifier::empty();
        }
    }
    cells_equal(&partial, &expected);
    assert_ne!(unfocused, focused);
    v.scroll = 0;
    v.inner.height = 4;
    let (partial, _, r) = draw(v, &[placement()], false);
    assert!(!r.iter().any(|r| r.kind == PicturePaintKind::Zoom));
    let mut expected = buffer();
    for y in 3..5 {
        for x in 7..11 {
            expected[(x, y)].fg = Color::Reset;
            expected[(x, y)].modifier = Modifier::empty();
        }
    }
    cells_equal(&partial, &expected);
    v.inner.height = 7;
    v.zoom = 1.001;
    assert!(!draw(v, &[placement()], false)
        .2
        .iter()
        .any(|r| r.kind == PicturePaintKind::Zoom));
    v.zoom = 1.002;
    assert!(draw(v, &[placement()], false)
        .2
        .iter()
        .any(|r| r.kind == PicturePaintKind::Zoom));
}

#[test]
fn overlay_signature_tracks_geometry_focus_zoom_and_departure() {
    let v = values();
    let p = placement();
    let (_, base, _) = draw(v, &[p], false);
    assert!(base.is_some());
    let mut other = p;
    other.url = "mermaid://two";
    assert_ne!(base, draw(v, &[other], false).1);
    let mut other = v;
    other.focused_ordinal = Some(3);
    assert_ne!(base, draw(other, &[p], false).1);
    other = v;
    other.zoom = 2.0;
    assert_ne!(base, draw(other, &[p], false).1);
    other = v;
    other.inner.x += 1;
    assert_ne!(base, draw(other, &[p], false).1);
    let mut hidden = p;
    hidden.visual_row = 30;
    assert_eq!(base, draw(v, &[p, hidden], false).1);
    assert_eq!(None, draw(v, &[hidden], false).1);
}

#[test]
fn pending_visible_reservation_does_not_change_signature_when_encoded() {
    let (pending, sig, _) = draw(values(), &[placement()], false);
    let (ready, encoded, _) = draw(values(), &[placement()], true);
    assert!(sig.is_some());
    assert_eq!(sig, encoded);
    cells_equal(&pending, &ready);
}

#[test]
fn overlay_move_clears_once_and_worker_results_redraw_without_input() {
    let mut moved = crate::inline_images::OverlayMoved::new();
    let p = placement();
    let mut v = values();
    for (scroll, expected) in [
        (0, false),
        (0, false),
        (1, true),
        (1, false),
        (20, true),
        (21, false),
    ] {
        v.scroll = scroll;
        moved.begin();
        if let Some(sig) = draw(v, &[p], false).1 {
            moved.note(sig);
        }
        moved.finish();
        assert_eq!(moved.take_moved(), expected, "scroll {scroll}");
    }
}
