use super::diff_frame::*;
use crate::scroll::ScrollWords;
use ratatui::{buffer::Buffer, layout::Rect};

#[test]
fn diff_titles_keep_scope_position_representation_and_missing_path() {
    let values = DiffTitleValues {
        path: Some("文書.md"),
        position: Some((2, 5)),
        scope: Some("since follow-start"),
        representation: None,
    };
    assert_eq!(
        values.title(false),
        " diff: 文書.md (2/5) · since follow-start "
    );
    assert_eq!(
        values.title(true),
        " diff ⇆: 文書.md (2/5) · since follow-start "
    );
    assert_eq!(
        DiffTitleValues {
            representation: Some("rendered"),
            ..values
        }
        .title(true),
        " diff ⟨rendered⟩: 文書.md (2/5) · since follow-start "
    );
    assert_eq!(
        DiffTitleValues {
            path: None,
            ..values
        }
        .title(true),
        " diff "
    );
}

#[test]
fn decorated_clamp_preserves_the_original_u16_conversions() {
    for wrap in [false, true] {
        let update = clamp_decorated_scroll(DecoratedScrollValues {
            vertical_limit: 70_000,
            max_line_cols: 65_540,
            inner_width: 2,
            viewport: 3,
            scroll: u16::MAX,
            hscroll: 100,
            wrap,
        });
        assert_eq!(update.scroll, u16::MAX);
        assert_eq!(update.hscroll, if wrap { 0 } else { 2 });
        assert_eq!(
            (update.extent.pos, update.extent.max, update.extent.viewport),
            (65_535, 65_535, 3)
        );
    }
}

#[test]
fn empty_source_keeps_offsets_and_resolves_auto_inside_the_border() {
    for (width, split) in [(91, false), (92, true)] {
        let area = Rect::new(3, 2, width, 5);
        let mut buf = Buffer::empty(area);
        let update = draw_source_diff(
            &mut buf,
            area,
            SourceDiffValues {
                lines: &[],
                extension: "",
                theme: "",
                layout: SourceDiffLayout::Auto { min_width: 90 },
                scroll: 9,
                hscroll: 11,
                title: DiffTitleValues::default(),
                empty_text: "(no changes)",
                scroll_words: ScrollWords {
                    all: "All",
                    top: "Top",
                    bot: "Bot",
                },
            },
        );
        assert_eq!(
            update,
            DiffDrawUpdate {
                scroll: 9,
                hscroll: 11,
                diff_split_drawn: split
            }
        );
        assert_eq!(buf.area, area);
        assert!(buf.content.iter().any(|c| c.symbol() == "n"));
    }
}

#[test]
fn binary_summary_keeps_unknown_sides_signs_and_thousands() {
    let words = BinarySummaryWords {
        binary_file: "binary",
        none: "none",
        same_size_different_content: "different",
        byte_unit: "B",
    };
    for (old_len, new_len, expected) in [
        (None, Some(2), "binary: none → 2"),
        (Some(2), None, "binary: 2 → none"),
        (None, None, "binary: none → none"),
        (Some(2), Some(2), "binary: 2 → 2 (different)"),
        (
            Some(1_234_569),
            Some(2),
            "binary: 1234569 → 2 (-1,234,567 B)",
        ),
    ] {
        assert_eq!(
            binary_summary(
                BinarySummaryValues {
                    same_bytes: false,
                    old_len,
                    new_len
                },
                words,
                |n| n.to_string()
            )
            .as_deref(),
            Some(expected)
        );
    }
    assert_eq!(format_byte_delta(0, "bytes"), "+0 bytes");
    assert_eq!(
        format_byte_delta(i64::MIN, "B"),
        "-9,223,372,036,854,775,808 B"
    );
}
