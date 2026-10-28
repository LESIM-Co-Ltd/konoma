//! Decoding the picture of a plugin's `View::Image` (`ImageView`), host-neutral: what a still, a
//! GIF, a vector source or a failure is made into. konoma (a picture plugin shown in a preview)
//! and the standalone app both call [`decode_picture_view`], so a picture looks the same in
//! either; each host only maps [`PictureView`] to what it keeps.
//!
//! This is the whole-picture counterpart of the canvas pictures ([`super::decode_one`]): the same
//! file door ([`super::files::open_picture_file`]), the same SVG decisions
//! ([`super::draw_plugin_svg`]) and the same decoders (`konoma_svg::image`, with their own size
//! limits), but a picture view keeps a GIF's frames whole and a vector source for sharp zoom.

use std::path::Path;
use std::sync::Arc;

use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::SvgTrust;

use super::files::open_picture_file;
use super::{draw_plugin_svg, SvgDraw};
use crate::view::{ImageData, ImageView};
use crate::ReadScope;

/// The frames of an animation with how long each is shown.
pub type PictureFrames = Vec<(image::DynamicImage, std::time::Duration)>;

/// What an `ImageView` was made into.
pub enum PictureView {
    /// A still picture.
    Still(image::DynamicImage),
    /// A raster of a vector source, with the SVG and how it is drawn again, kept so a zoom can
    /// redraw it at the density it needs (`ImageView::sharp_zoom`).
    Vector {
        img: image::DynamicImage,
        svg: Arc<Vec<u8>>,
        trust: SvgTrust,
    },
    /// An animation with more than one frame.
    Gif(PictureFrames),
    /// It cannot be shown, with the reason.
    Failed(ImageFailure),
}

/// Decode what a picture plugin returned for the file `path`.
///
/// `None` is a failure with no reason to show (the host falls back), or the job being cancelled
/// (`cancelled`), or raw pixels whose buffer does not match their size.
///
/// `default_raster_px` is the side an SVG is drawn at unless the view names its own
/// (`raster_max_px`). `trust_own_svg` is whether the host believes the plugin's `trusted` claim
/// on an SVG it returns (decided by where the plugin comes from); without it every SVG is drawn by
/// `svg`, and with no `base_dir` such an SVG reads no file. `scope` limits the files the view may
/// name.
///
/// `Encoded`, `Frames` and `Lazy` are not made by the picture plugins; a plugin that returns them
/// gets [`ImageFailure::UnsupportedFormat`] (`ImageData` is non-exhaustive: a kind unknown here is
/// refused the same way).
pub fn decode_picture_view(
    iv: ImageView,
    path: &Path,
    default_raster_px: u32,
    cancelled: &dyn Fn() -> bool,
    trust_own_svg: bool,
    scope: &ReadScope,
    svg: &dyn SvgDraw,
) -> Option<PictureView> {
    let max_px = iv.raster_max_px.unwrap_or(default_raster_px);
    match iv.data {
        // A file: the host tells an animation from a still by the first bytes, and decodes it
        // under its size limits and the shared decode gate. It is opened by the one door every
        // plugin-named picture file goes through (a regular file only, never a FIFO or a device,
        // opened without blocking; a relative path is relative to the previewed file's folder),
        // and decoded from that handle -- nothing opens the path a second time. (No 64 MiB read
        // cap here: a large still is bounded by the decoders' own limits.)
        ImageData::File(p) => Some(match open_picture_file(path, &p, scope) {
            Ok((file, named)) => decode_picture_handle(file, &named),
            Err(why) => PictureView::Failed(why),
        }),
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => {
            let (img, trust) = match draw_plugin_svg(
                &data,
                base_dir,
                trusted,
                path,
                max_px,
                scope,
                trust_own_svg,
                svg,
                cancelled,
            ) {
                Ok(drawn) => drawn,
                Err(None) => return None,
                Err(Some(why)) => return Some(PictureView::Failed(why)),
            };
            Some(if iv.sharp_zoom {
                PictureView::Vector {
                    img,
                    svg: Arc::new(data.to_vec()),
                    trust,
                }
            } else {
                PictureView::Still(img)
            })
        }
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            // The plugin handed over the only reference: the buffer is taken, not copied.
            let raw = Arc::try_unwrap(pixels).unwrap_or_else(|shared| (*shared).clone());
            let img = image::RgbaImage::from_raw(width, height, raw)?;
            Some(PictureView::Still(image::DynamicImage::ImageRgba8(img)))
        }
        _ => Some(PictureView::Failed(ImageFailure::UnsupportedFormat)),
    }
}

/// Decode the opened picture file: an animation when its first bytes say GIF (and it has more
/// than one frame), otherwise a still.
fn decode_picture_handle(mut file: std::fs::File, named: &Path) -> PictureView {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 6];
    let gif = file.read_exact(&mut head).is_ok() && &head[..4] == b"GIF8";
    if file.seek(SeekFrom::Start(0)).is_err() {
        return PictureView::Failed(ImageFailure::Corrupt);
    }
    if gif {
        // A single-frame / non-animated GIF is shown as a still image.
        let frames = file
            .try_clone()
            .ok()
            .and_then(konoma_svg::image::decode_gif_file);
        if let Some(frames) = frames {
            return PictureView::Gif(frames);
        }
        if file.seek(SeekFrom::Start(0)).is_err() {
            return PictureView::Failed(ImageFailure::Corrupt);
        }
    }
    match konoma_svg::image::decode_static_file_why(file, Some(named)) {
        Ok(img) => PictureView::Still(img),
        Err(why) => PictureView::Failed(why),
    }
}

#[cfg(test)]
#[path = "picture_view_tests.rs"]
mod tests;
