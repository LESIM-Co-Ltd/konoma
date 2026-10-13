// SVG preview: the rasterizer, the font database and the guard rails live in the `konoma-svg`
// crate (re-exported here, so `crate::preview::svg::...` paths are unchanged). What stays in the
// host is the entry points for SVG that konoma did not write: they are drawn by a supervised child
// process (`svg_proc`), which the crate cannot depend on, and they need to know where the remote
// image cache is (`app::md_remote_cache_path`), so a downloaded SVG reads no local file.

use std::path::Path;

use image::DynamicImage;

pub use konoma_svg::svg::*;

use super::svg_proc;

/// The directory that holds downloaded (remote) images: a file directly inside it is drawn with no
/// file readable beside it. Passed to every entry point below, never registered globally.
fn remote_cache_dir() -> Option<std::path::PathBuf> {
    crate::app::md_remote_cache_path("").and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Draws untrusted SVG bytes in the supervised child process.
fn run_in_child(
    data: &[u8],
    base: Option<&Path>,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError> {
    #[cfg(test)]
    if svg_proc::child_exe().is_none() {
        // Unit tests of the callers run the same guarded drawing in this process; the tests of
        // the child itself start the real binary through `svg_proc::run_with`.
        return Ok(rasterize_guarded(data, base, max_px)?);
    }
    let req = svg_proc::Request { data, base, max_px };
    match svg_proc::run(&req, cancelled) {
        Ok(img) => Ok(img),
        Err(svg_proc::RunError::Failed(f)) => Err(RasterError::Failed(f)),
        Err(svg_proc::RunError::Cancelled) => Err(RasterError::Cancelled),
    }
}

/// Rasterize the SVG file at `path` with a max side of `max_px` in a supervised child process.
/// Blocks: call from a worker thread. `cancelled` is polled while waiting; when it turns true the
/// child is stopped and the result is `Err(RasterError::Cancelled)`.
pub fn rasterize(
    path: &Path,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError> {
    rasterize_via(
        path,
        max_px,
        cancelled,
        remote_cache_dir().as_deref(),
        &run_in_child,
    )
}

/// Rasterize SVG bytes that konoma did not write (a file, a git blob, a cached download), in a
/// supervised child process. `path` is only where relative references resolve from. Blocks.
///
/// The cheap checks that need no parser (size, nesting depth) run first, so an obviously
/// hostile file is refused without starting a process.
pub fn rasterize_untrusted(
    data: &[u8],
    path: &Path,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError> {
    rasterize_untrusted_via(
        data,
        path,
        max_px,
        cancelled,
        remote_cache_dir().as_deref(),
        &run_in_child,
    )
}

/// Rasterize SVG bytes that are not a file on disk (a picture embedded in a Word / OpenDocument
/// file), in a supervised child process. The document has no directory its references could be
/// relative to, so the drawing reads **no** file (`<image href="/Users/x/private.png">` paints
/// nothing). Blocks.
pub fn rasterize_embedded(
    data: &[u8],
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError> {
    rasterize_embedded_via(data, max_px, cancelled, &run_in_child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use resvg::{tiny_skia, usvg};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Draws `svg` against `db` through the same `resvg::render` call the previewer makes, and
    /// returns the raw pixels.
    ///
    /// The fallback tests need a fact that markup cannot fake: whether glyphs really landed on the
    /// page. **Counting *painted* pixels would not show it** — a mermaid label sits inside a filled
    /// node box, so every pixel it touches is already opaque and the alpha count is identical with
    /// and without a font (measured: 8,949 either way). What separates them is *colour*, so the
    /// tests compare two rasters of the same SVG instead.
    fn raster(svg: &str, db: Arc<usvg::fontdb::Database>) -> Vec<u8> {
        let opt = usvg::Options {
            fontdb: db,
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(svg, &opt).expect("the renderer's own SVG must parse");
        let size = tree.size();
        let w = (size.width().ceil() as u32).max(1);
        let h = (size.height().ceil() as u32).max(1);
        let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap should allocate");
        resvg::render(
            &tree,
            tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        pixmap.take()
    }

    /// The one path neither this machine nor CI can reach on its own, so it is built by hand:
    /// a database that never saw a system font, which is what a slim container has.
    #[test]
    fn the_embedded_face_draws_a_diagram_on_a_machine_with_no_fonts() {
        use crate::preview::mermaid::render;
        use crate::preview::mermaid::text_metrics::{TextMetrics, FONT_SIZE};

        // Without the fallback: exactly the state the Linux CI runner was in. The gate the
        // renderer asks before it builds anything says "do not draw", and 45 tests said so too.
        let bare = usvg::fontdb::Database::new();
        assert_eq!(bare.len(), 0, "the stand-in machine has no fonts at all");
        assert!(!resolves_sans_serif(&bare));
        assert!(
            TextMetrics::resolve(Arc::new(bare)).is_err(),
            "with no font the renderer's gate must refuse — this is the regression"
        );

        // With it: the same empty machine, through the very function `shared_fontdb` calls.
        let mut fallback = usvg::fontdb::Database::new();
        assert!(
            install_fallback_sans_serif(&mut fallback),
            "the embedded face must register when nothing else answers to sans-serif"
        );
        assert!(
            resolves_sans_serif(&fallback),
            "and sans-serif now resolves"
        );
        let fallback = Arc::new(fallback);

        // Measurement — the input every box size is derived from.
        //
        // "It resolved" is not enough to check here, and the difference is not academic: swapping
        // the embedded file for `KaTeX_Size4-Regular.ttf` — a face of brackets, with no letters at
        // all — resolves, measures, and even moves pixels. So what is asserted is that the face
        // has the glyphs, and that the width it reports is the width of five *letters* (measured:
        // 29.4px) rather than of five stand-ins. `text_metrics` falls back to the `.notdef` glyph
        // for a character nothing in the database can draw, which for this face is a quarter of an
        // em — 17.5px for these five — so the lower bound has to sit above that to say anything.
        let metrics = TextMetrics::resolve(fallback.clone())
            .expect("the gate must pass once the embedded face is registered");
        assert!(
            !metrics.has_missing_glyph("Start"),
            "the embedded face must carry Latin glyphs, not merely answer to the family name"
        );
        let width = metrics.measure("Start", FONT_SIZE);
        assert!(
            (20.0..60.0).contains(&width),
            "five letters at {FONT_SIZE}px should measure like letters, not like stand-ins: {width}px"
        );

        // Drawing — through the renderer's own entry point, so what is checked is what a caller
        // gets rather than a hand-written `<text>`.
        let svg = render::render("flowchart LR\n  A[Start] --> B[Stop]\n", "dark")
            .expect("the diagram must lay out");
        let unlabelled = raster(&svg, Arc::new(usvg::fontdb::Database::new()));
        let labelled = raster(&svg, fallback);
        assert!(
            unlabelled.iter().any(|&b| b != 0),
            "the boxes and arrows are drawn either way — otherwise the comparison means nothing"
        );
        let (labelled_px, _) = labelled.as_chunks::<4>();
        let (unlabelled_px, _) = unlabelled.as_chunks::<4>();
        let changed = labelled_px
            .iter()
            .zip(unlabelled_px)
            .filter(|(a, b)| a != b)
            .count();
        // Two five-letter labels at 14px cover a few hundred pixels; the bound only has to be far
        // enough above zero that a stray antialiasing pixel could not carry the test.
        assert!(
            changed > 100,
            "the labels must put glyphs on the page: only {changed} pixels differ between the \
             diagram drawn with the embedded face and the same diagram drawn with no font at all"
        );
    }

    // The helpers below are the ones the crate's own tests use for the same purpose.

    /// A 20x20 solid-red PNG written to `dir/name`.
    fn red_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        path
    }

    /// An SVG that draws the image at `href` over a 20x20 canvas.
    fn svg_with_image(href: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image xlink:href="{href}" width="20" height="20"/></svg>"#
        )
    }

    fn center_alpha(img: &image::DynamicImage) -> u8 {
        let rgba = img.to_rgba8();
        rgba.get_pixel(rgba.width() / 2, rgba.height() / 2)[3]
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn a_cached_remote_svg_reads_no_file_but_a_local_one_still_does() {
        // The cache root is a sandbox directory, never the real ~/.cache.
        let root = unique_tmp("konoma_svg_remote");
        crate::test_support::set_test_cache_root(root.to_path_buf());
        let cache = root.join("konoma").join("remote-images");
        std::fs::create_dir_all(&cache).unwrap();
        let png = red_png(&root, "secret.png");
        let svg = svg_with_image(&png.display().to_string());
        // A downloaded SVG (stored in the remote-image cache, no extension) draws nothing.
        let remote = cache.join("0123456789abcdef");
        std::fs::write(&remote, &svg).unwrap();
        assert_eq!(center_alpha(&rasterize(&remote, 100, &never).unwrap()), 0);
        // The same bytes as a file the user placed elsewhere still read the picture.
        let local = root.join("mine.svg");
        std::fs::write(&local, &svg).unwrap();
        assert_eq!(center_alpha(&rasterize(&local, 100, &never).unwrap()), 255);
    }
}
