//! `open_file` with the real plugins of the `konoma-media` program: a picture or a video is
//! recognised by its content (the plugins bind `image/*` and `video/*`), whatever its name.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_plugin::app::{open_file, AppError};
use konoma_plugin::{Plugin, TermCaps};
use konoma_test_support::unique_tmp;

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(name)
}

fn open(path: &Path) -> Result<konoma_plugin::app::Opened, AppError> {
    let plugins: Vec<Arc<dyn Plugin>> =
        konoma_media::program().into_iter().map(Arc::from).collect();
    open_file(plugins, path, "en", TermCaps::new())
}

fn copy_as(tag: &str, from: &str, to: &str) -> (konoma_test_support::TmpDir, PathBuf) {
    let d = unique_tmp(tag);
    std::fs::create_dir_all(d.as_path()).unwrap();
    let p = d.as_path().join(to);
    std::fs::copy(sample(from), &p).unwrap();
    (d, p)
}

fn id_of(path: &Path) -> String {
    match open(path) {
        Ok(o) => o.plugin_id,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

#[test]
fn a_png_a_jpg_and_a_gif_open_as_images() {
    for name in ["sample.png", "sample.jpg", "sample.gif"] {
        assert_eq!(id_of(&sample(name)), "image", "{name}");
    }
}

#[test]
fn a_picture_with_no_extension_or_the_wrong_one_opens_as_an_image() {
    for (from, to) in [
        ("sample.png", "pic"),
        ("sample.png", "pic.txt"),
        ("sample.png", "pic.dat"),
        ("sample.gif", "anim"),
        ("sample.gif", "anim.txt"),
        ("sample.jpg", "photo"),
    ] {
        let (_d, p) = copy_as("media_open_pic", from, to);
        assert_eq!(id_of(&p), "image", "{from} as {to}");
    }
}

#[test]
fn a_video_opens_as_a_video_with_or_without_its_name() {
    assert_eq!(id_of(&sample("sample.mp4")), "video");
    let (_d, p) = copy_as("media_open_vid", "sample.mp4", "clip");
    assert_eq!(id_of(&p), "video");
    let (_d, p) = copy_as("media_open_vid2", "sample.mp4", "clip.dat");
    assert_eq!(id_of(&p), "video");
}

#[test]
fn a_text_file_named_as_a_picture_is_not_opened_as_one() {
    let d = unique_tmp("media_open_fake");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let p = d.as_path().join("fake.zzz");
    std::fs::write(&p, "plain words\n").unwrap();
    assert!(matches!(open(&p), Err(AppError::NotHandled { .. })));
}
