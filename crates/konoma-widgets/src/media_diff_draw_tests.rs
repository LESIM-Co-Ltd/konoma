use super::*;
use crate::diff_frame::DiffTitleValues;
use crate::picture_overlay::{PicturePaintKind, PicturePaintRequest};
use ratatui::buffer::Buffer;

#[test]
fn media_geometry_uses_intrinsic_dimensions() {
    let side = |key: &str, natural_px| MediaSideView::Picture {
        key: key.into(),
        natural_px,
        bytes: 83_250,
        page_count: Some(7),
    };
    let state = MediaDiffRenderState::Ready {
        old: side("old", (80, 64)),
        new: side("new", (40, 32)),
        same_bytes: true,
    };
    let area = Rect::new(3, 2, 44, 13);
    let mut buf = Buffer::empty(area);
    let mut requests = Vec::new();
    draw_media_diff(
        &mut buf,
        area,
        MediaDiffDrawValues {
            state: &state,
            cell_px: (8, 16),
            layout: MediaDiffLayout::Auto,
            page: 2,
            title: DiffTitleValues::default(),
            old_label: "base",
            new_label: "live",
            words: MediaDiffWords {
                before: "before",
                after: "after",
                identical: "same",
                absent_old: "absent old",
                absent_new: "absent new",
                cannot_display: "failed",
                page_missing: "missing",
            },
            format_size: |n| n.to_string(),
        },
        |key, r, _| {
            requests.push((key.to_owned(), r));
            false
        },
    );
    let request = |target: Rect| PicturePaintRequest {
        target,
        cols: target.width,
        rows: target.height,
        row_offset: 0,
        visible_rows: target.height,
        kind: PicturePaintKind::Normal,
    };
    assert_eq!(
        requests,
        vec![
            ("old".into(), request(Rect::new(9, 7, 10, 4))),
            ("new".into(), request(Rect::new(33, 8, 5, 2)))
        ]
    );
    assert_eq!(buf.area, area);
    let MediaDiffRenderState::Ready { old, .. } = state else {
        panic!("ready")
    };
    assert_eq!(old.natural_px(), Some((80, 64)));
    assert_eq!(MediaSideView::Absent.natural_px(), None);
    assert_eq!(MediaSideView::PageMissing.natural_px(), None);
    assert_eq!(
        MediaSideView::Failed {
            reason: "why".into()
        }
        .natural_px(),
        None
    );
}

#[test]
fn media_raster_box_preserves_64px_quantization_and_zero_floor() {
    for (rect, cell_px, expected) in [
        (Rect::new(3, 2, 23, 11), (8, 16), (192, 192)),
        (Rect::new(0, 0, 0, 0), (0, 0), (64, 64)),
        (Rect::new(0, 0, 8, 4), (8, 16), (64, 64)),
        (Rect::new(0, 0, 9, 5), (8, 16), (128, 128)),
        (
            Rect::new(0, 0, u16::MAX, u16::MAX),
            (8, 16),
            (524_288, 1_048_576),
        ),
    ] {
        assert_eq!(media_raster_box(rect, cell_px), expected);
    }
}
