//! Decoding one picture of a canvas: a still, an animation, a file or an SVG, under the decoders'
//! own size limits. The decoders, the size caps and the frame delay are fixed here; only the
//! [`CanvasPolicy`] (scope, cancellation, SVG drawer) varies by host.

use std::path::{Path, PathBuf};

use image::DynamicImage;
use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::{SvgTrust, INLINE_IMAGE_MAX_SIDE};

use super::budget::pixel_bytes_of;
use super::files::read_picture_file;
use super::{CanvasDecoded, CanvasPolicy, SvgDraw, MIN_FRAME_DELAY};
use crate::view::ImageData;
use crate::ReadScope;

/// Draw the SVG a plugin returned, and say how it was vouched for (what a host that redraws it at
/// another size needs). `Err(None)` is a failure with no reason to show (an SVG the host's own
/// in-process drawing could not render); `Err(Some(why))` carries the reason.
///
/// `trusted` is the plugin's claim; `trust_own_svg` is whether the host believes it
/// ([`CanvasPolicy::trust_own_svg`]). Without both every SVG goes through `svg`; with no
/// `base_dir` such an SVG reads no file. The SVG's relative references are read from the directory
/// the plugin names, so that directory is a path the plugin gave: outside the read scope the SVG is
/// refused with the scope's reason (not silently drawn without its pictures). One that cannot be
/// resolved holds nothing to read, so the SVG names no directory. Every file the drawing reads is
/// judged again against the scope (`roots`), whatever the SVG says.
#[allow(clippy::too_many_arguments)]
pub fn draw_plugin_svg(
    data: &[u8],
    base_dir: Option<PathBuf>,
    trusted: bool,
    shown: &Path,
    max_px: u32,
    scope: &ReadScope,
    trust_own_svg: bool,
    svg: &dyn SvgDraw,
    cancelled: &dyn Fn() -> bool,
) -> Result<(DynamicImage, SvgTrust), Option<ImageFailure>> {
    let base_dir = match base_dir {
        Some(d) if scope.is_restricted() => match scope.resolve(&d) {
            Ok(real) => Some(real),
            Err(crate::ScopeError::Outside) => return Err(Some(ImageFailure::OutsideReadDirs)),
            Err(_) => None,
        },
        other => other,
    };
    let roots = scope.dirs();
    // Relative references inside the SVG resolve from the directory the plugin names.
    let base = match (&base_dir, shown.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => shown.to_path_buf(),
    };
    if trusted && trust_own_svg {
        let img = konoma_svg::svg::rasterize_trusted(data, &base, max_px).ok_or(None)?;
        Ok((img, SvgTrust::Trusted(base)))
    } else if base_dir.is_some() {
        // A file the user opened: drawn by the host's supervised drawer.
        match svg.untrusted(data, &base, roots, max_px, cancelled) {
            Ok(img) => Ok((
                img,
                match roots {
                    Some(r) => SvgTrust::FileWithin(base, r.into()),
                    None => SvgTrust::File(base),
                },
            )),
            Err(why) => Err(Some(why.into())),
        }
    } else {
        // The plugin names no directory: nothing to read relative to, so the drawing reads no file
        // at all (the shown file's directory is not the SVG's).
        match svg.embedded(data, max_px, cancelled) {
            Ok(img) => Ok((img, SvgTrust::Embedded)),
            Err(why) => Err(Some(why.into())),
        }
    }
}

/// Decode one still picture (no animation): the reason when it cannot be shown.
pub(super) fn decode_still(
    data: &ImageData,
    path: &Path,
    policy: &CanvasPolicy,
) -> Result<DynamicImage, ImageFailure> {
    match data {
        ImageData::File(p) => {
            let read = read_picture_file(path, p, policy.scope)?;
            // What was read is the host's own (the file passed the scope above): its `base_dir`
            // is the folder of that file, inside the scope. The scope still goes on, because the
            // SVG's own references are judged against it as the drawing reads them.
            decode_still(&read, path, policy)
        }
        ImageData::Encoded(b) => {
            konoma_svg::image::decode_static_bytes_capped_why(b, INLINE_IMAGE_MAX_SIDE)
        }
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => draw_plugin_svg(
            data,
            base_dir.clone(),
            *trusted,
            path,
            policy.raster_px,
            policy.scope,
            policy.trust_own_svg,
            policy.svg,
            policy.cancelled,
        )
        .map(|(img, _)| img)
        .map_err(|why| why.unwrap_or(ImageFailure::Corrupt)),
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            // The buffer is shared with the plugin's answer (which the host keeps), so it is copied.
            image::RgbaImage::from_raw(*width, *height, (**pixels).clone())
                .map(DynamicImage::ImageRgba8)
                .ok_or(ImageFailure::Corrupt)
        }
        ImageData::Frames(_) | ImageData::Lazy(_) => Err(ImageFailure::UnsupportedFormat),
        // A kind this host does not know (`ImageData` is non-exhaustive for other crates).
        #[allow(unreachable_patterns)]
        _ => Err(ImageFailure::UnsupportedFormat),
    }
}

/// Decode one picture of a canvas, an animation included.
pub fn decode_one(data: &ImageData, path: &Path, policy: &CanvasPolicy) -> CanvasDecoded {
    // A file is read once, from the handle that was opened (`read_picture_file`), and decoded from
    // memory: a GIF file is an animation like GIF bytes are.
    let read;
    let data = if let ImageData::File(p) = data {
        match read_picture_file(path, p, policy.scope) {
            Ok(d) => {
                read = d;
                &read
            }
            Err(why) => return CanvasDecoded::Failed(why),
        }
    } else {
        data
    };
    let animated = match data {
        // A GIF is an animation when it has more than one frame; anything else (a single frame,
        // a damaged one) falls through to the still decode, as an inline Markdown image does.
        ImageData::Encoded(b) if b.starts_with(b"GIF8") => {
            konoma_svg::image::decode_gif_bytes_inline(b).map(|(frames, _)| frames)
        }
        _ => None,
    };
    if let Some(frames) = animated.filter(|f| !f.is_empty()) {
        return CanvasDecoded::Frames(frames);
    }
    if let ImageData::Frames(list) = data {
        if list.is_empty() {
            return CanvasDecoded::Failed(ImageFailure::UnsupportedFormat);
        }
        let mut bytes = 0u64;
        let mut frames = Vec::with_capacity(list.len().min(256));
        for (f, d) in list {
            if (policy.cancelled)() {
                return CanvasDecoded::Failed(ImageFailure::Cancelled);
            }
            match decode_still(f, path, policy) {
                Ok(img) => {
                    bytes += pixel_bytes_of(&img);
                    if bytes > konoma_svg::image::MAX_GIF_BYTES_INLINE as u64 {
                        return CanvasDecoded::Failed(ImageFailure::TooLarge);
                    }
                    frames.push((img, (*d).max(MIN_FRAME_DELAY)));
                }
                Err(why) => return CanvasDecoded::Failed(why),
            }
        }
        return CanvasDecoded::Frames(frames);
    }
    match decode_still(data, path, policy) {
        Ok(img) => CanvasDecoded::Still(img),
        Err(why) => CanvasDecoded::Failed(why),
    }
}
