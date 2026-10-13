//! Media thumbnails and pages for konoma's previews: PDF pages (pure Rust, with the macOS
//! QuickLook fallback) and video thumbnails (pure-Rust H.264 / HEVC decoding of one keyframe, with
//! ffmpeg as the fallback for everything else).

pub mod pdf;
pub mod video;
