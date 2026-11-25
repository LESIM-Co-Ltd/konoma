//! `decode_picture_view`: every kind of `ImageData` a picture view can carry, and the limits and
//! reasons the decoding settles. (The mapping to konoma's media payload is tested in the host.)

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{DynamicImage, Rgba, RgbaImage};

use super::*;
use crate::view::{ImageData, ImageView};
use crate::ReadScope;

/// An SVG drawer that records which of its two ways was used and returns a 3 x 2 picture.
#[derive(Default)]
struct Recorder {
    untrusted: Cell<u32>,
    embedded: Cell<u32>,
}

impl SvgDraw for Recorder {
    fn untrusted(
        &self,
        _: &[u8],
        _: &Path,
        _: Option<&[PathBuf]>,
        max_px: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        self.untrusted.set(self.untrusted.get() + 1);
        Ok(DynamicImage::ImageRgba8(RgbaImage::new(3, max_px.min(2))))
    }

    fn embedded(
        &self,
        _: &[u8],
        max_px: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        self.embedded.set(self.embedded.get() + 1);
        Ok(DynamicImage::ImageRgba8(RgbaImage::new(max_px.min(2), 3)))
    }
}

/// A drawer that refuses (the drawing process failed or was cancelled).
struct Refuses(konoma_svg::svg::RasterError);

impl SvgDraw for Refuses {
    fn untrusted(
        &self,
        _: &[u8],
        _: &Path,
        _: Option<&[PathBuf]>,
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        Err(self.0)
    }

    fn embedded(
        &self,
        _: &[u8],
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        Err(self.0)
    }
}

const SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="red"/></svg>"#;

/// A fresh folder, removed when the guard drops.
fn tmp(prefix: &str) -> konoma_test_support::TmpDir {
    let dir = konoma_test_support::unique_tmp(prefix);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn view(data: ImageData) -> ImageView {
    ImageView::new(data)
}

fn svg_data(base_dir: Option<PathBuf>, trusted: bool) -> ImageData {
    ImageData::Svg {
        data: Arc::from(SVG),
        base_dir,
        trusted,
    }
}

fn never() -> bool {
    false
}

fn run(
    iv: ImageView,
    shown: &Path,
    trust_own: bool,
    scope: &ReadScope,
    svg: &dyn SvgDraw,
) -> Option<PictureView> {
    decode_picture_view(iv, shown, 100, &never, trust_own, scope, svg)
}

fn free() -> ReadScope {
    ReadScope::unrestricted()
}

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([1, 2, 3, 255])))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn gif_bytes(frames: usize) -> Vec<u8> {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame};
    let mut gif = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut gif);
        for i in 0..frames {
            enc.encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(8, 8, Rgba([(i * 90) as u8, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(50, 1),
            ))
            .unwrap();
        }
    }
    gif
}

fn failed(got: Option<PictureView>) -> ImageFailure {
    match got {
        Some(PictureView::Failed(why)) => why,
        Some(_) => panic!("expected a failure, got a picture"),
        None => panic!("expected a failure, got None"),
    }
}

fn still(got: Option<PictureView>) -> DynamicImage {
    match got {
        Some(PictureView::Still(img)) => img,
        Some(PictureView::Vector { .. }) => panic!("expected a still, got a vector"),
        Some(PictureView::Gif(_)) => panic!("expected a still, got a GIF"),
        Some(PictureView::Failed(why)) => panic!("expected a still, failed: {why:?}"),
        None => panic!("expected a still, got None"),
    }
}

#[test]
fn a_still_file_is_decoded_from_the_opened_handle() {
    let dir = tmp("pv_still");
    let p = dir.join("a.png");
    std::fs::write(&p, png_bytes(5, 4)).unwrap();
    let img = still(run(
        view(ImageData::File(p.clone())),
        &p,
        false,
        &free(),
        &Recorder::default(),
    ));
    assert_eq!((img.width(), img.height()), (5, 4));
}

#[test]
fn a_relative_file_is_relative_to_the_shown_files_folder() {
    let dir = tmp("pv_rel");
    std::fs::write(dir.join("b.png"), png_bytes(7, 3)).unwrap();
    let shown = dir.join("doc.txt");
    let img = still(run(
        view(ImageData::File("b.png".into())),
        &shown,
        false,
        &free(),
        &Recorder::default(),
    ));
    assert_eq!((img.width(), img.height()), (7, 3));
}

#[test]
fn a_gif_file_with_several_frames_is_an_animation() {
    let dir = tmp("pv_gif");
    let p = dir.join("a.gif");
    std::fs::write(&p, gif_bytes(3)).unwrap();
    match run(
        view(ImageData::File(p.clone())),
        &p,
        false,
        &free(),
        &Recorder::default(),
    ) {
        Some(PictureView::Gif(frames)) => assert_eq!(frames.len(), 3),
        _ => panic!("a GIF with three frames is an animation"),
    }
}

#[test]
fn a_one_frame_gif_file_is_a_still() {
    let dir = tmp("pv_gif1");
    let p = dir.join("a.gif");
    std::fs::write(&p, gif_bytes(1)).unwrap();
    let img = still(run(
        view(ImageData::File(p.clone())),
        &p,
        false,
        &free(),
        &Recorder::default(),
    ));
    assert_eq!((img.width(), img.height()), (8, 8));
}

#[test]
fn a_damaged_file_is_corrupt_and_a_missing_one_too() {
    let dir = tmp("pv_bad");
    let p = dir.join("a.png");
    let mut b = png_bytes(5, 4);
    b.truncate(30);
    std::fs::write(&p, b).unwrap();
    let r = Recorder::default();
    assert_eq!(
        failed(run(
            view(ImageData::File(p.clone())),
            &p,
            false,
            &free(),
            &r
        )),
        ImageFailure::Corrupt
    );
    let gone = dir.join("gone.png");
    assert_eq!(
        failed(run(
            view(ImageData::File(gone.clone())),
            &gone,
            false,
            &free(),
            &r
        )),
        ImageFailure::Corrupt
    );
}

#[test]
fn bytes_that_are_not_an_image_are_unsupported() {
    let dir = tmp("pv_text");
    // No extension to name a format by, and no signature in the bytes.
    let p = dir.join("a.dat");
    std::fs::write(&p, b"this is not a picture at all").unwrap();
    assert_eq!(
        failed(run(
            view(ImageData::File(p.clone())),
            &p,
            false,
            &free(),
            &Recorder::default()
        )),
        ImageFailure::UnsupportedFormat
    );
}

#[test]
fn a_picture_over_the_size_limit_is_too_large_by_its_header() {
    let dir = tmp("pv_big");
    let p = dir.join("wide.png");
    // One pixel high, one past the longest side: tiny to write, refused before anything is allocated.
    std::fs::write(&p, png_bytes(konoma_svg::image::MAX_IMAGE_SIDE + 1, 1)).unwrap();
    assert_eq!(
        failed(run(
            view(ImageData::File(p.clone())),
            &p,
            false,
            &free(),
            &Recorder::default()
        )),
        ImageFailure::TooLarge
    );
}

#[test]
fn a_fifo_is_refused_without_being_opened() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tmp("pv_fifo");
    let p = dir.join("pipe.gif");
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // Opening a FIFO for reading with no writer would block: this returning at all is the point.
    assert_eq!(
        failed(run(
            view(ImageData::File(p.clone())),
            &p,
            false,
            &free(),
            &Recorder::default()
        )),
        ImageFailure::Corrupt
    );
}

#[test]
fn a_file_outside_the_read_scope_is_refused_and_the_shown_file_is_allowed() {
    let dir = tmp("pv_scope");
    let inside = dir.join("in");
    let outside = dir.join("out");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("s.png");
    std::fs::write(&secret, png_bytes(2, 2)).unwrap();
    let shown = inside.join("a.png");
    std::fs::write(&shown, png_bytes(3, 3)).unwrap();
    let scope = ReadScope::within([&inside]);
    let r = Recorder::default();
    assert_eq!(
        failed(run(
            view(ImageData::File(secret)),
            &shown,
            false,
            &scope,
            &r
        )),
        ImageFailure::OutsideReadDirs
    );
    let img = still(run(
        view(ImageData::File(shown.clone())),
        &shown,
        false,
        &scope,
        &r,
    ));
    assert_eq!(img.width(), 3);
}

#[test]
fn encoded_frames_and_lazy_are_not_made_by_picture_plugins() {
    #[derive(Debug)]
    struct L;
    impl crate::input::LazyResource for L {
        fn natural_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn render(
            &self,
            _: &crate::input::ResourceReq,
            _: &crate::Cancel,
        ) -> Result<ImageData, crate::Failure> {
            panic!("a picture view never renders a lazy resource")
        }
    }
    let r = Recorder::default();
    let shown = Path::new("/x/a.png");
    for data in [
        ImageData::Encoded(Arc::from(png_bytes(2, 2))),
        ImageData::Frames(vec![]),
        ImageData::Lazy(Arc::new(L)),
    ] {
        assert_eq!(
            failed(run(view(data), shown, false, &free(), &r)),
            ImageFailure::UnsupportedFormat
        );
    }
    assert_eq!((r.untrusted.get(), r.embedded.get()), (0, 0));
}

#[test]
fn rgba_pixels_become_a_still_and_a_wrong_sized_buffer_is_none() {
    let shown = Path::new("/x/a.png");
    let r = Recorder::default();
    let ok = ImageData::Rgba {
        width: 2,
        height: 3,
        pixels: Arc::new(vec![9; 2 * 3 * 4]),
    };
    let img = still(run(view(ok), shown, false, &free(), &r));
    assert_eq!((img.width(), img.height()), (2, 3));
    for (w, h, n) in [(2u32, 3u32, 2 * 3 * 4 - 1), (4, 4, 0)] {
        let bad = ImageData::Rgba {
            width: w,
            height: h,
            pixels: Arc::new(vec![0; n]),
        };
        assert!(run(view(bad), shown, false, &free(), &r).is_none());
    }
    // A buffer longer than the size needs is accepted (`RgbaImage::from_raw` takes the first
    // width x height pixels); this is how a picture has always been taken.
    let long = ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: Arc::new(vec![0; 8]),
    };
    assert_eq!(still(run(view(long), shown, false, &free(), &r)).width(), 1);
}

#[test]
fn a_shared_rgba_buffer_is_copied_and_the_plugins_stays_intact() {
    let pixels = Arc::new(vec![5u8; 4]);
    let keep = Arc::clone(&pixels);
    let data = ImageData::Rgba {
        width: 1,
        height: 1,
        pixels,
    };
    let img = still(run(
        view(data),
        Path::new("/x/a.png"),
        false,
        &free(),
        &Recorder::default(),
    ));
    assert_eq!(img.width(), 1);
    assert_eq!(*keep, vec![5u8; 4]);
}

#[test]
fn an_untrusted_svg_with_a_base_dir_goes_to_the_supervised_drawer() {
    let r = Recorder::default();
    let shown = Path::new("/x/a.svg");
    let got = run(
        view(svg_data(Some("/x".into()), true)),
        shown,
        false, // the host does not believe the claim
        &free(),
        &r,
    );
    assert_eq!((r.untrusted.get(), r.embedded.get()), (1, 0));
    assert_eq!(still(got).width(), 3);
}

#[test]
fn an_svg_with_no_base_dir_reads_no_file() {
    let r = Recorder::default();
    let got = run(
        view(svg_data(None, false)),
        Path::new("/x/a.svg"),
        false,
        &free(),
        &r,
    );
    assert_eq!((r.untrusted.get(), r.embedded.get()), (0, 1));
    assert_eq!(still(got).height(), 3);
}

#[test]
fn a_trusted_claim_is_believed_only_when_the_host_believes_it() {
    let r = Recorder::default();
    let shown = Path::new("/x/a.svg");
    // Both: drawn in this process, no drawer used, and the vector keeps the trust.
    let mut iv = view(svg_data(Some("/x".into()), true));
    iv.sharp_zoom = true;
    match run(iv, shown, true, &free(), &r) {
        Some(PictureView::Vector { img, trust, .. }) => {
            assert_eq!((img.width(), img.height()), (100, 50));
            assert!(matches!(trust, SvgTrust::Trusted(_)));
        }
        _ => panic!("a trusted SVG is drawn in this process"),
    }
    assert_eq!((r.untrusted.get(), r.embedded.get()), (0, 0));
    // The plugin claims nothing: the host's belief alone does not skip the drawer.
    let _ = run(
        view(svg_data(Some("/x".into()), false)),
        shown,
        true,
        &free(),
        &r,
    );
    assert_eq!(r.untrusted.get(), 1);
}

#[test]
fn sharp_zoom_keeps_the_svg_and_how_it_is_drawn_again() {
    let r = Recorder::default();
    let shown = Path::new("/x/a.svg");
    let mut iv = view(svg_data(Some("/x".into()), false));
    iv.sharp_zoom = true;
    match run(iv, shown, false, &free(), &r) {
        Some(PictureView::Vector { svg, trust, .. }) => {
            assert_eq!(svg.as_slice(), SVG);
            assert_eq!(trust, SvgTrust::File(PathBuf::from("/x/a.svg")));
        }
        _ => panic!("sharp zoom keeps the vector"),
    }
    let mut iv = view(svg_data(None, false));
    iv.sharp_zoom = true;
    match run(iv, shown, false, &free(), &r) {
        Some(PictureView::Vector { trust, .. }) => assert_eq!(trust, SvgTrust::Embedded),
        _ => panic!("sharp zoom keeps the vector"),
    }
    // Without sharp_zoom the same SVG is a plain still.
    assert!(matches!(
        run(view(svg_data(None, false)), shown, false, &free(), &r),
        Some(PictureView::Still(_))
    ));
}

#[test]
fn the_raster_size_is_the_views_own_or_the_default() {
    let r = Recorder::default();
    let shown = Path::new("/x/a.svg");
    // The recorder draws min(max_px, 2) high: 1 for the view's own size, 2 for the default 100.
    let mut iv = view(svg_data(Some("/x".into()), false));
    iv.raster_max_px = Some(1);
    assert_eq!(still(run(iv, shown, false, &free(), &r)).height(), 1);
    let iv = view(svg_data(Some("/x".into()), false));
    assert_eq!(still(run(iv, shown, false, &free(), &r)).height(), 2);
}

#[test]
fn an_svg_directory_outside_the_scope_is_refused_with_the_scope_reason() {
    let dir = tmp("pv_svgscope");
    let inside = dir.join("in");
    let outside = dir.join("out");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let scope = ReadScope::within([&inside]);
    let r = Recorder::default();
    assert_eq!(
        failed(run(
            view(svg_data(Some(outside), false)),
            &inside.join("a.svg"),
            false,
            &scope,
            &r
        )),
        ImageFailure::OutsideReadDirs
    );
    assert_eq!(r.untrusted.get(), 0);
}

#[test]
fn a_drawer_failure_is_its_reason_and_a_cancelled_drawing_too() {
    let shown = Path::new("/x/a.svg");
    let why = konoma_svg::svg_guard::SvgFail::TooLarge;
    let got = run(
        view(svg_data(Some("/x".into()), false)),
        shown,
        false,
        &free(),
        &Refuses(konoma_svg::svg::RasterError::Failed(why)),
    );
    assert_eq!(failed(got), ImageFailure::Svg(why));
    let got = run(
        view(svg_data(None, false)),
        shown,
        false,
        &free(),
        &Refuses(konoma_svg::svg::RasterError::Cancelled),
    );
    assert_eq!(failed(got), ImageFailure::Cancelled);
}

#[test]
fn an_svg_this_process_cannot_draw_has_no_reason() {
    let iv = view(ImageData::Svg {
        data: Arc::from(&b"not an svg"[..]),
        base_dir: Some("/x".into()),
        trusted: true,
    });
    assert!(run(
        iv,
        Path::new("/x/a.svg"),
        true,
        &free(),
        &Recorder::default()
    )
    .is_none());
}

#[test]
fn the_jobs_cancellation_reaches_the_drawer() {
    struct Asks(Cell<Option<bool>>);
    impl SvgDraw for Asks {
        fn untrusted(
            &self,
            _: &[u8],
            _: &Path,
            _: Option<&[PathBuf]>,
            _: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            self.0.set(Some(cancelled()));
            Err(konoma_svg::svg::RasterError::Cancelled)
        }

        fn embedded(
            &self,
            _: &[u8],
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            unreachable!()
        }
    }
    for flag in [true, false] {
        let d = Asks(Cell::new(None));
        let got = decode_picture_view(
            view(svg_data(Some("/x".into()), false)),
            Path::new("/x/a.svg"),
            100,
            &|| flag,
            false,
            &free(),
            &d,
        );
        assert_eq!(d.0.get(), Some(flag));
        assert_eq!(failed(got), ImageFailure::Cancelled);
    }
}
