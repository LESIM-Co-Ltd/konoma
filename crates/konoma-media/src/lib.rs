//! konoma's picture plugins: `image`, `svg`, `pdf` and `video`.
//!
//! Each returns a [`konoma_plugin::view::ImageView`] and the host draws it (decoding, animation,
//! zoom, pan and the terminal's graphics protocol are the host's). What differs per plugin:
//!
//! - `image`: the file itself. The host decodes it (it also tells a GIF from a still by the first
//!   bytes), so a large picture is decoded under the host's size limits and cancellation.
//! - `svg`: the SVG bytes, marked **untrusted**: the host draws them in a supervised child process
//!   and redraws sharply when zoomed.
//! - `pdf`: pages (`Layout.pages`), one picture per page, drawn here with `hayro`.
//! - `video`: one representative frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::DynamicImage;
use konoma_plugin::view::{ImageData, ImageView, Layout, PageLayout, View, ViewReq};
use konoma_plugin::{
    t, Bind, Failure, FailureKind, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, Session,
    SettingDecl, Value, Verb, VerbDecl, ViewCx,
};

/// The ids of the four plugins.
pub const IMAGE_ID: &str = "image";
pub const SVG_ID: &str = "svg";
pub const PDF_ID: &str = "pdf";
pub const VIDEO_ID: &str = "video";

/// The default longest side, in pixels, of a drawn SVG (`[plugins.svg] max_px`).
pub const SVG_MAX_PX: i64 = 800;

/// `edit` opens the user's editor in the terminal; `open` hands the file to the OS default
/// application as a GUI program.
fn default_verbs(m: Manifest) -> Manifest {
    m.verb(
        VerbDecl::new(Verb::Edit)
            .launcher(Launcher::Editor)
            .mode(LaunchMode::Terminal)
            .line_token(),
    )
    .verb(
        VerbDecl::new(Verb::Open)
            .launcher(Launcher::OsDefault)
            .mode(LaunchMode::Gui),
    )
}

fn manifest(id: &str, name: konoma_plugin::Tr) -> Manifest {
    Manifest::new(id, env!("CARGO_PKG_VERSION")).name(name)
}

/// The declarations of the `svg` plugin. Registered before `image`: an SVG file is also
/// `image/svg+xml` to the content sniffer, and the SVG plugin must win.
pub fn svg_manifest() -> Manifest {
    default_verbs(
        manifest(SVG_ID, t!(en: "SVG", ja: "SVG"))
            .bind(Bind::files(["*.svg"]))
            .setting(SettingDecl::new(
                "max_px",
                Value::Int(SVG_MAX_PX),
                t!(en: "Longest side of the drawing, in pixels", ja: "描画の長辺(px)"),
            )),
    )
}

/// The declarations of the `image` plugin.
pub fn image_manifest() -> Manifest {
    default_verbs(manifest(IMAGE_ID, t!(en: "Image", ja: "画像")).bind(Bind::mime("image/*")))
}

/// The declarations of the `video` plugin.
pub fn video_manifest() -> Manifest {
    default_verbs(manifest(VIDEO_ID, t!(en: "Video", ja: "動画")).bind(Bind::mime("video/*")))
}

/// The declarations of the `pdf` plugin.
pub fn pdf_manifest() -> Manifest {
    default_verbs(manifest(PDF_ID, t!(en: "PDF", ja: "PDF")).bind(Bind::files(["*.pdf"])))
}

/// The path of a file input, or why the input is not one.
fn file_of(input: &Input) -> Result<PathBuf, Failure> {
    match input {
        Input::File { path, .. } => Ok(path.clone()),
        _ => Err(Failure::unsupported(t!(
            en: "this plugin opens files only",
            ja: "このプラグインはファイルだけを開きます",
        ))),
    }
}

/// A picture as raw RGBA pixels.
fn rgba_view(img: DynamicImage) -> ImageView {
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    ImageView::new(ImageData::Rgba {
        width,
        height,
        pixels: Arc::new(rgba.into_raw()),
    })
}

/// A session that is one file and nothing else (the plugin reads the file each time it is asked).
struct FileSession {
    path: PathBuf,
}

macro_rules! plugin {
    ($(#[$doc:meta])* $name:ident, $manifest:ident, $session:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl Plugin for $name {
            fn manifest(&self) -> Manifest {
                $manifest()
            }

            fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                Ok(Box::new($session::new(file_of(&input)?)))
            }
        }
    };
}

// ---- image ------------------------------------------------------------------------------------

struct ImageSession(FileSession);

impl ImageSession {
    fn new(path: PathBuf) -> Self {
        ImageSession(FileSession { path })
    }
}

impl Session for ImageSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        // The host decodes the file: under its size limits, on its worker, and it sees the first
        // bytes to tell an animation from a still.
        Ok(View::Image(ImageView::new(ImageData::File(
            self.0.path.clone(),
        ))))
    }
}

plugin!(
    /// The image plugin (PNG, JPEG, GIF, WebP...).
    ImagePlugin,
    image_manifest,
    ImageSession
);

// ---- svg --------------------------------------------------------------------------------------

struct SvgSession(FileSession);

impl SvgSession {
    fn new(path: PathBuf) -> Self {
        SvgSession(FileSession { path })
    }
}

/// The failure of reading an SVG file (too big, not a regular file, unreadable). The text is the
/// one the host shows for the same reason.
fn svg_read_failure(why: konoma_svg::svg_guard::SvgFail) -> Failure {
    use konoma_svg::svg_guard::SvgFail;
    match why {
        SvgFail::TooLarge => Failure::too_large(t!(
            en: "[svg] not drawn: the file is too large",
            ja: "[svg] 描画しません: ファイルが大きすぎます",
        )),
        _ => Failure::corrupt(t!(
            en: "[svg] not drawn: the file is damaged or is not a valid SVG",
            ja: "[svg] 描画しません: ファイルが壊れているか、SVG として正しくありません",
        )),
    }
}

impl Session for SvgSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let data = konoma_svg::svg::read_limited(&self.0.path).map_err(svg_read_failure)?;
        let max_px = cx.settings.int("max_px", SVG_MAX_PX).clamp(1, 16384) as u32;
        Ok(View::Image(ImageView {
            data: ImageData::Svg {
                data: Arc::from(data),
                base_dir: self.0.path.parent().map(Path::to_path_buf),
                // A file the user opened: the host draws it in a supervised child process.
                trusted: false,
            },
            sharp_zoom: true,
            raster_max_px: Some(max_px),
        }))
    }
}

plugin!(
    /// The SVG plugin.
    SvgPlugin,
    svg_manifest,
    SvgSession
);

// ---- pdf --------------------------------------------------------------------------------------

struct PdfSession {
    path: PathBuf,
    /// The page count, or `None` when the file could not be parsed (one page is still tried).
    count: Option<u32>,
}

fn pdf_failure() -> Failure {
    Failure::unsupported(t!(
        en: "[pdf] cannot render — use a kitty-graphics terminal (or this PDF is encrypted/corrupt)",
        ja: "[PDF] 表示不可 — kitty graphics 対応端末を使ってください(または暗号化/破損した PDF です)",
    ))
}

impl Session for PdfSession {
    fn layout(&self) -> Layout {
        Layout {
            pages: Some(PageLayout::Separate {
                count: self.count.map(|n| n as usize),
                titles: Vec::new(),
            }),
            ..Layout::single()
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::new(
                FailureKind::Cancelled,
                t!(en: "cancelled", ja: "中断しました"),
            ));
        }
        let last = self.count.map_or(0, |n| n.saturating_sub(1)) as usize;
        let page = req.page.unwrap_or(0).min(last) as u32 + 1;
        // `hayro` (pure Rust) first; macOS's bundled qlmanage/sips only as the page-1 fallback and
        // only when the user allows external programs.
        konoma_media_core::pdf::render_page(&self.path, page, cx.external)
            .map(|img| View::Image(rgba_view(img)))
            .ok_or_else(pdf_failure)
    }
}

/// The PDF plugin: pages are the host's page keys (`J` / `K`).
#[derive(Clone, Copy, Debug, Default)]
pub struct PdfPlugin;

impl Plugin for PdfPlugin {
    fn manifest(&self) -> Manifest {
        pdf_manifest()
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let path = file_of(&input)?;
        // Counting pages reads the page tree in pure Rust; no external program is involved.
        let count = konoma_media_core::pdf::page_count(&path);
        Ok(Box::new(PdfSession { path, count }))
    }
}

// ---- video ------------------------------------------------------------------------------------

struct VideoSession(FileSession);

impl VideoSession {
    fn new(path: PathBuf) -> Self {
        VideoSession(FileSession { path })
    }
}

impl Session for VideoSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if cx.cancel.is_cancelled() {
            return Err(Failure::new(
                FailureKind::Cancelled,
                t!(en: "cancelled", ja: "中断しました"),
            ));
        }
        // H.264/H.265 in mp4 and mkv are decoded in pure Rust; ffmpegthumbnailer / ffmpeg are the
        // fallback for everything else, and only when the user allows external programs.
        konoma_media_core::video::thumbnail(&self.0.path, cx.external)
            .map(|img| View::Image(rgba_view(img)))
            .ok_or_else(|| {
                Failure::external(t!(
                    en: "[video] no thumbnail — install ffmpegthumbnailer or ffmpeg (and use a kitty-graphics terminal)",
                    ja: "[動画] サムネイル不可 — ffmpegthumbnailer か ffmpeg を導入してください(kitty graphics 対応端末が必要)",
                ))
            })
    }
}

plugin!(
    /// The video plugin: one representative frame, no playback.
    VideoPlugin,
    video_manifest,
    VideoSession
);
