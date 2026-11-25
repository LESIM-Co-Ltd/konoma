//! Scoped, read-only facts from the real media worker, cache and picture reservations.
use crate::host::project::media_diff::{
    media_side_view, MediaDiffOutcome, MediaDiffWant, MediaDiffWorker,
};
use konoma_widgets::{
    inline_images::InlineImageCache,
    media_diff::{MediaDiffLayout, MediaSideView},
    picture_overlay::PicturePaintRequest,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaCacheSnapshot {
    pub svg_bytes: Option<Vec<u8>>,
    pub key: PathBuf,
    pub decoded_px: Option<(u32, u32)>,
    pub frames: usize,
    pub frame: usize,
    pub encoding: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaStateSnapshot {
    pub page: Option<u32>,
    pub layout: Option<MediaDiffLayout>,
    pub cell_px: Option<(u32, u32)>,
    pub held: Option<String>,
    pub sides: Option<(MediaSideView, MediaSideView)>,
    pub pending: bool,
    pub queued: Option<MediaDiffWant>,
    pub deadline: bool,
    pub cache: Vec<MediaCacheSnapshot>,
    pub paints: Vec<(String, PicturePaintRequest)>,
}

pub fn observe_cache(cache: &InlineImageCache) -> Vec<MediaCacheSnapshot> {
    let mut entries: Vec<_> = cache
        .cache
        .iter()
        .filter(|(key, _)| konoma_widgets::media_diff::is_media_diff_url(&key.to_string_lossy()))
        .map(|(key, entry)| MediaCacheSnapshot {
            svg_bytes: entry.svg.as_deref().cloned(),
            key: key.clone(),
            decoded_px: entry.decoded.as_ref().map(|p| (p.width(), p.height())),
            frames: entry.frames.len(),
            frame: entry.idx,
            encoding: entry.enc_inflight,
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries
}

impl MediaStateSnapshot {
    pub fn worker(
        worker: &MediaDiffWorker,
        path: &Path,
        page: u32,
        layout: MediaDiffLayout,
        cell_px: Option<(u32, u32)>,
        deadline: bool,
    ) -> Self {
        let held = worker.outcome_for(path);
        Self {
            page: Some(page),
            layout: Some(layout),
            cell_px,
            held: held.map(|o| format!("{o:?}")),
            sides: match held {
                Some(MediaDiffOutcome::Ready { old, new, .. }) => {
                    Some((media_side_view(old), media_side_view(new)))
                }
                _ => None,
            },
            pending: worker.busy(),
            queued: worker.queued().clone(),
            deadline,
            ..Default::default()
        }
    }
}
