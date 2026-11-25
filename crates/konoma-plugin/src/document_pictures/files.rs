//! A document picture as a file: where its URL points, how big it is, and how it is decoded.
//!
//! The same rules in every host. An SVG is drawn through the host's [`SvgDraw`] (konoma draws it
//! in a supervised child process, the standalone app may draw it in process).

use std::path::{Path, PathBuf};

use image::DynamicImage;
use konoma_svg::image::ImageFailure;
use konoma_svg::svg::RasterError;
use konoma_svg::svg_guard::SvgFail;

use super::remote::md_remote_cache_path;
use crate::canvas_pictures::SvgDraw;

/// Longest side, in pixels, that an inline Markdown image is kept at after decoding (the value
/// every host shares: [`konoma_widgets::inline_images::INLINE_IMAGE_MAX_SIDE`]).
pub const MD_IMAGE_MAX_SIDE: u32 = konoma_widgets::inline_images::INLINE_IMAGE_MAX_SIDE;

/// The file a document image URL names on disk, **without** checking that it exists or touching
/// the filesystem. `None` for an empty URL, a `data:` URL and a remote (`http(s)://`) URL; a remote
/// one names the download cache rather than a path in the tree, which is [`resolve_md_image_path`]'s
/// business. Relative paths are resolved against the document's directory.
///
/// Split out of [`resolve_md_image_path`] for the caller that must recognize a picture which has
/// just been **deleted**: a filesystem event has to be matched against the images the open
/// document draws from, and a deleted file no longer resolves.
pub fn md_image_local_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    let u = url.trim();
    if u.is_empty() || konoma_widgets::markdown::is_remote_image_url(u) {
        return None;
    }
    if u.to_ascii_lowercase().starts_with("data:") {
        return None;
    }
    let u = u.strip_prefix("file://").unwrap_or(u);
    let p = PathBuf::from(u);
    Some(match base {
        Some(b) if !p.is_absolute() => b.join(p),
        _ => p,
    })
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    /// How many times this thread resolved a document picture to a file (each is a stat): lets a
    /// test hold a host's document preview to one resolve per picture per frame.
    pub static MD_IMAGE_RESOLVES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Resolve a document image URL to a local file path. A remote (`http(s)://`) URL resolves to its
/// download-cache path, but only once it has been fetched (so callers treat a cached remote image
/// exactly like a local one). `data:` URLs return None. Returns the path only if the file exists.
pub fn resolve_md_image_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-hooks"))]
    MD_IMAGE_RESOLVES.with(|c| c.set(c.get() + 1));
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    if konoma_widgets::markdown::is_remote_image_url(u) {
        let p = md_remote_cache_path(u)?;
        return p.is_file().then_some(p);
    }
    let p = md_image_local_path(u, base)?;
    p.is_file().then_some(p)
}

/// Pixel dimensions of a cached inline-image file, accepting both raster formats and SVG (parsed
/// cheaply, without rasterizing). None if the file is neither a known raster image nor an SVG.
pub fn md_image_dims(path: &Path) -> Option<(u32, u32)> {
    md_image_dims_why(path).ok()
}

/// [`md_image_dims`] that says why an SVG has no size, when there is something to tell: `Err(Some)`
/// is a refusal (too large, nested too deeply, ...), `Err(None)` is "not an image we can size".
/// An SVG's size is read from its root element alone, so this is cheap whatever the file holds.
pub fn md_image_dims_why(path: &Path) -> Result<(u32, u32), Option<SvgFail>> {
    if let Some(d) = konoma_svg::image::dimensions(path) {
        return Ok(d);
    }
    konoma_svg::svg::intrinsic_size(path).map_err(refusal_worth_telling)
}

/// A failure the user should be told about (everything except "this is not a valid SVG", which for
/// a file that failed as an image too is just a picture that cannot be shown).
pub fn refusal_worth_telling(why: SvgFail) -> Option<SvgFail> {
    (why != SvgFail::Invalid).then_some(why)
}

/// Decode an inline-image file to an image, rasterizing SVG (at `svg_max_px`, through `svg`) when
/// the raster decoders reject it (READMEs are full of SVG badges/logos). An image the raster
/// decoders refuse for being too large is not offered to the SVG rasterizer, nor is a file that
/// cannot begin as one (a damaged PNG); any other refusal is, and the raster decoder's reason
/// stands when the file is not a valid SVG either. Blocks for as long as the drawing takes: call it
/// from a worker. With `roots` set an SVG reads only files inside them (`None` is no limit).
pub fn md_decode_image_within(
    path: &Path,
    svg_max_px: u32,
    roots: Option<&[PathBuf]>,
    svg: &dyn SvgDraw,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, ImageFailure> {
    match konoma_svg::image::decode_static_capped_why(path, MD_IMAGE_MAX_SIDE) {
        Ok(img) => Ok(img),
        Err(e @ (ImageFailure::TooLarge | ImageFailure::Cancelled)) => Err(e),
        // A damaged PNG or JPEG is not an SVG either, and drawing it would cost a process hand-off.
        Err(e) if !konoma_svg::svg::file_can_be_svg(path) => Err(e),
        Err(e) => {
            let drawn = konoma_svg::svg::read_limited(path)
                .map_err(RasterError::from)
                .and_then(|data| svg.untrusted(&data, path, roots, svg_max_px, cancelled));
            match drawn {
                Ok(img) => Ok(img),
                Err(RasterError::Failed(SvgFail::Invalid)) => Err(e),
                Err(f) => Err(f.into()),
            }
        }
    }
}

/// [`md_decode_image_within`] for a picture held in memory (inside a Word / OpenDocument file):
/// the same limits and reasons. An embedded SVG is not a file, so it is drawn with no directory to
/// read from (`<image href>` inside it paints nothing).
pub fn md_decode_bytes_why(
    bytes: &[u8],
    svg_max_px: u32,
    svg: &dyn SvgDraw,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, ImageFailure> {
    match konoma_svg::image::decode_static_bytes_capped_why(bytes, MD_IMAGE_MAX_SIDE) {
        Ok(img) => Ok(img),
        Err(e @ (ImageFailure::TooLarge | ImageFailure::Cancelled)) => Err(e),
        Err(e) if !konoma_svg::svg::can_begin_svg(bytes) => Err(e),
        Err(e) => match svg.embedded(bytes, svg_max_px, cancelled) {
            Ok(img) => Ok(img),
            Err(RasterError::Failed(SvgFail::Invalid)) => Err(e),
            Err(f) => Err(f.into()),
        },
    }
}

/// Whether the file at `path` is a GIF, judged by the leading bytes (the magic, not the
/// extension). The file may be a FIFO or a device the user selected in a tree: only a regular
/// file is opened, and never blocking, so this is safe on the UI thread too.
pub fn looks_like_gif(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = crate::canvas_pictures::files::open_regular_picture(path) else {
        return false;
    };
    let mut head = [0u8; 6];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    &head[..4] == b"GIF8" // GIF87a / GIF89a
}
