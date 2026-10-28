//! The arithmetic of the per-answer budget: what a picture costs in decoded bytes, decodes, input
//! bytes and header-announced pixels, so the worker can decide before it decodes.

use std::path::Path;

use super::files::{open_picture_file, resolve_picture_path};
use super::CanvasDecoded;
use crate::view::ImageData;

/// The bytes of pixels a decoded image holds: in the image's own pixel format (3 bytes for an
/// RGB8, 8 for an RGBA16, 16 for a float image), the measure of `MdImgEntry::pixel_bytes`, so the
/// worker, the landing and the cache all count the same.
pub(super) fn pixel_bytes_of(i: &image::DynamicImage) -> u64 {
    use image::GenericImageView;
    let (w, h) = i.dimensions();
    u64::from(w) * u64::from(h) * u64::from(i.color().bytes_per_pixel())
}

/// The pixel bytes a decoded picture holds in the cache.
pub fn decoded_bytes(d: &CanvasDecoded) -> u64 {
    match d {
        CanvasDecoded::Still(i) => pixel_bytes_of(i),
        CanvasDecoded::Frames(f) => f.iter().map(|(i, _)| pixel_bytes_of(i)).sum(),
        CanvasDecoded::Failed(_) => 0,
    }
}

/// A copy of a decoded picture (for a second id that holds the same content: decoded once).
pub(super) fn copy_decoded(d: &CanvasDecoded) -> CanvasDecoded {
    match d {
        CanvasDecoded::Still(i) => CanvasDecoded::Still(i.clone()),
        CanvasDecoded::Frames(f) => CanvasDecoded::Frames(f.clone()),
        CanvasDecoded::Failed(w) => CanvasDecoded::Failed(*w),
    }
}

/// How many decodes the picture `data` costs the budget of an answer. An animation given as
/// `ImageData::Frames` costs one per frame that runs a decoder: an SVG (drawn in a child
/// process), a file (read, and possibly an SVG) or an `Encoded` image (a full decode of whatever
/// size its header admits, however small its bytes); only a ready `Rgba` frame costs nothing
/// (bounded by the input budget). Any picture costs at least one.
pub(super) fn decode_cost(data: &ImageData) -> usize {
    fn frame(d: &ImageData) -> usize {
        match d {
            ImageData::Svg { .. } | ImageData::File(_) | ImageData::Encoded(_) => 1,
            ImageData::Frames(f) => f.iter().map(|(d, _)| frame(d)).sum(),
            _ => 0,
        }
    }
    match data {
        ImageData::Frames(f) => f.iter().map(|(d, _)| frame(d)).sum::<usize>().max(1),
        _ => 1,
    }
}

/// The bytes of input a decode of `data` reads (what the per-answer input budget counts).
pub(super) fn input_bytes(data: &ImageData, shown: &Path) -> u64 {
    match data {
        ImageData::File(p) => {
            std::fs::metadata(resolve_picture_path(shown, p)).map_or(0, |m| m.len())
        }
        ImageData::Encoded(b) => b.len() as u64,
        ImageData::Svg { data, .. } => data.len() as u64,
        ImageData::Rgba { pixels, .. } => pixels.len() as u64,
        ImageData::Frames(f) => f.iter().map(|(d, _)| input_bytes(d, shown)).sum(),
        ImageData::Lazy(_) => 0,
        // A kind this host does not know (`ImageData` is non-exhaustive for other crates).
        #[allow(unreachable_patterns)]
        _ => 0,
    }
}

/// The pixels of work a decode of `data` is announced to cost by its **header alone** (nothing is
/// decoded: `ImageReader::into_dimensions` reads the first bytes, and the frames of a GIF are
/// counted from its block structure), and whether that is all there is to know:
///
/// A `File` outside the read `scope` (or one that does not open) costs `(0, false)` and is never
/// opened: konoma reads no byte of it, not even for the budget.
///
/// - `Encoded` / `File` (PNG, JPEG, GIF, WebP and the rest the `image` crate sniffs): width x
///   height, times the frame count for a GIF. A damaged header costs 0 here -- the decoder refuses
///   it cheaply and says why.
/// - `Rgba`: 0, there is nothing to decode (a copy, bounded by the input budget).
/// - `Frames`: the sum over the frames.
/// - `Svg` (and a file that is an SVG): **unknown before drawing** -- its size is not in a header
///   but in the drawing, which the supervised child process bounds (`raster_max_px` per side). The
///   second value is `true` for it: the caller adds the pixels of the picture it got back.
#[cfg(any(test, feature = "test-hooks"))]
pub fn header_pixels(data: &ImageData, shown: &Path, scope: &crate::ReadScope) -> (u64, bool) {
    header_pixels_within(data, shown, scope, &|| false)
}

/// [`header_pixels`] that stops counting the frames of a GIF once `stale` says the answer was
/// superseded (the count then falls back to one frame).
pub(super) fn header_pixels_within(
    data: &ImageData,
    shown: &Path,
    scope: &crate::ReadScope,
    stale: &dyn Fn() -> bool,
) -> (u64, bool) {
    use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
    fn reader_px<R: std::io::BufRead + Seek>(mut r: R, stale: &dyn Fn() -> bool) -> u64 {
        let mut head = [0u8; 10];
        let n = r.read(&mut head).unwrap_or(0);
        let gif = head[..n].starts_with(b"GIF8");
        if r.seek(SeekFrom::Start(0)).is_err() {
            return 0;
        }
        if gif {
            // The logical screen size sits at a fixed place; the frames are counted below.
            if n < 10 {
                return 0;
            }
            let px = u64::from(u16::from_le_bytes([head[6], head[7]]))
                * u64::from(u16::from_le_bytes([head[8], head[9]]));
            let frames = konoma_svg::image::count_gif_frames(&mut r, 5_000, stale);
            return px.saturating_mul(frames.map_or(1, |f| f.max(1)) as u64);
        }
        let Ok(reader) = image::ImageReader::new(&mut r).with_guessed_format() else {
            return 0;
        };
        let Ok((w, h)) = reader.into_dimensions() else {
            return 0;
        };
        u64::from(w) * u64::from(h)
    }
    match data {
        ImageData::Encoded(b) => (reader_px(Cursor::new(&b[..]), stale), false),
        ImageData::File(p) => match open_picture_file(shown, p, scope) {
            Ok((file, _)) => {
                let mut r = BufReader::new(file);
                let mut head = [0u8; 512];
                let n = r.read(&mut head).unwrap_or(0);
                if konoma_svg::svg::can_begin_svg(&head[..n]) {
                    return (0, true);
                }
                if r.seek(SeekFrom::Start(0)).is_err() {
                    return (0, false);
                }
                (reader_px(r, stale), false)
            }
            Err(_) => (0, false),
        },
        ImageData::Svg { .. } => (0, true),
        ImageData::Frames(f) => {
            // The sum of the frames; the pixels of an SVG frame are still unknown, so the whole
            // is "unknown before drawing" as soon as one frame is.
            let parts: Vec<(u64, bool)> = f
                .iter()
                .map(|(d, _)| header_pixels_within(d, shown, scope, stale))
                .collect();
            (
                parts.iter().map(|p| p.0).fold(0u64, u64::saturating_add),
                parts.iter().any(|p| p.1),
            )
        }
        _ => (0, false),
    }
}

/// The pixels a decoded picture holds (what an SVG costs once it is drawn).
pub(super) fn decoded_pixels(d: &CanvasDecoded) -> u64 {
    use image::GenericImageView;
    let px = |i: &image::DynamicImage| {
        let (w, h) = i.dimensions();
        u64::from(w) * u64::from(h)
    };
    match d {
        CanvasDecoded::Still(i) => px(i),
        CanvasDecoded::Frames(f) => f.iter().map(|(i, _)| px(i)).sum(),
        CanvasDecoded::Failed(_) => 0,
    }
}

/// The bytes of memory the source `data` of a picture holds (a file is not held: it is read when
/// decoded).
pub fn source_bytes(data: &ImageData) -> u64 {
    match data {
        ImageData::File(_) | ImageData::Lazy(_) => 0,
        ImageData::Encoded(b) => b.len() as u64,
        ImageData::Svg { data, .. } => data.len() as u64,
        ImageData::Rgba { pixels, .. } => pixels.len() as u64,
        ImageData::Frames(f) => f.iter().map(|(d, _)| source_bytes(d)).sum(),
        // A kind this host does not know (`ImageData` is non-exhaustive for other crates).
        #[allow(unreachable_patterns)]
        _ => 0,
    }
}
