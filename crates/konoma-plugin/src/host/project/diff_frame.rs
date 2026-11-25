//! Light adapters from project configuration and computed outcomes to widget drawing values.
pub use konoma_widgets::diff_frame::*;

pub fn source_diff_layout(layout: konoma_host_core::project::DiffLayout) -> SourceDiffLayout {
    use konoma_host_core::project::{DiffLayout, DIFF_AUTO_MIN_WIDTH};
    match layout {
        DiffLayout::Unified => SourceDiffLayout::Unified,
        DiffLayout::Split => SourceDiffLayout::Split,
        DiffLayout::Auto => SourceDiffLayout::Auto {
            min_width: DIFF_AUTO_MIN_WIDTH,
        },
    }
}

pub fn binary_summary_values(
    outcome: &super::media_diff::MediaDiffOutcome,
) -> Option<BinarySummaryValues> {
    use super::media_diff::{MediaDiffOutcome, MediaDiffSide};
    let (same_bytes, old_len, new_len) = match outcome {
        MediaDiffOutcome::Summary {
            same_bytes,
            old_len,
            new_len,
            ..
        } => (*same_bytes, *old_len, *new_len),
        MediaDiffOutcome::Ready {
            same_bytes,
            old,
            new,
            ..
        } => {
            let len = |side: &MediaDiffSide| match side {
                MediaDiffSide::Picture(p) => Some(p.bytes),
                _ => None,
            };
            (*same_bytes, len(old), len(new))
        }
        MediaDiffOutcome::Unavailable => return None,
    };
    Some(BinarySummaryValues {
        same_bytes,
        old_len,
        new_len,
    })
}
