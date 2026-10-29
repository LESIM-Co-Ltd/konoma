//! The standalone programs (`konoma-media <file>` and the other `export!` programs) pick the
//! plugin for a file the way konoma does. For each program, over a corpus of the files in
//! `samples/` and files made here (a picture with no extension or the wrong one, an empty file,
//! a binary file with a NUL byte, text): when konoma's choice is a plugin the program carries,
//! the program picks the same one; when it is not, the program refuses the file, and says
//! "reads as text" exactly when konoma shows the file as text.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_plugin::app::{pick_plugin, AppError};
use konoma_plugin::Plugin;

use super::resolve::resolve_kind;
use crate::preview::PreviewKind;
use crate::test_support::unique_tmp;

type Program = (&'static str, fn() -> Vec<Box<dyn Plugin>>);

/// Every plugin program of the workspace that `export!`s a list from its library. The test
/// `every_program_is_in_the_list` keeps this in step with the `main.rs` files.
fn programs() -> Vec<Program> {
    vec![
        ("konoma-code", konoma_code::program),
        ("konoma-markdown", konoma_markdown::program),
        ("konoma-math", konoma_math::program),
        ("konoma-mermaid", konoma_mermaid::program),
        ("konoma-office", konoma_office::program),
        ("konoma-table", konoma_table::program),
        ("konoma-media", konoma_media::program),
    ]
}

fn arcs(make: fn() -> Vec<Box<dyn Plugin>>) -> Vec<Arc<dyn Plugin>> {
    make().into_iter().map(Arc::from).collect()
}

fn ids(make: fn() -> Vec<Box<dyn Plugin>>) -> Vec<String> {
    make().iter().map(|p| p.manifest().id).collect()
}

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("samples")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The files of `samples/` and the files made in `dir`.
fn corpus(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(&samples_dir(), &mut files);
    let from_samples = files.len();
    assert!(from_samples >= 40, "samples/ shrank: {from_samples}");
    let sample = |name: &str| std::fs::read(samples_dir().join(name)).unwrap();
    let (png, jpg, gif, mp4) = (
        sample("sample.png"),
        sample("sample.jpg"),
        sample("sample.gif"),
        sample("sample.mp4"),
    );
    let mut made: Vec<(&str, Vec<u8>)> = vec![
        ("png_noext", png.clone()),
        ("png_as.txt", png.clone()),
        ("png_as.dat", png.clone()),
        ("png_as.md", png.clone()),
        ("jpg_noext", jpg.clone()),
        ("jpg_as.bin", jpg.clone()),
        ("gif_noext", gif.clone()),
        ("gif_as.txt", gif.clone()),
        ("mp4_noext", mp4.clone()),
        ("mp4_as.dat", mp4.clone()),
        ("empty_noext", Vec::new()),
        ("empty.zzz", Vec::new()),
        ("empty.png", Vec::new()),
        ("nul_binary.zzz", vec![1, 2, 0, 3, 4, 0, 255]),
        ("nul_binary_noext", vec![0, 0, 0, 0]),
        ("text_noext", b"plain words\nsecond line\n".to_vec()),
        ("text.zzz", b"plain words\n".to_vec()),
        ("text_as.png", b"not really a picture\n".to_vec()),
        ("text_as.mp4", b"not really a video\n".to_vec()),
    ];
    // A NUL byte after the first 8 KiB does not make a file binary.
    let mut late_nul = vec![b'a'; 9000];
    late_nul[8500] = 0;
    made.push(("late_nul.zzz", late_nul));
    for (name, bytes) in made {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        files.push(p);
    }
    files
}

/// What konoma settles on for `path`, with the standard plugins and no user rules.
fn host_kind(path: &Path) -> PreviewKind {
    resolve_kind(&[], &[], true, path, None)
}

/// The files a program takes by a lower-priority binding, because the plugin konoma picks first
/// is not in the program: (program, file name, the plugin the program picks instead).
const LOWER_PRIORITY: &[(&str, &str, &str)] = &[("konoma-media", "png_as.md", "image")];

/// Compare one program over the corpus; returns how many files it picked a plugin for.
fn check_program(name: &str, make: fn() -> Vec<Box<dyn Plugin>>, files: &[PathBuf]) -> usize {
    let own = ids(make);
    let mut picked = 0;
    for f in files {
        let got = pick_plugin(arcs(make), f);
        let ctx = format!("{name} on {}", f.display());
        match host_kind(f) {
            PreviewKind::Plugin { id, .. } if own.contains(&id) => {
                assert_eq!(got.as_deref().ok(), Some(id.as_str()), "{ctx}: {got:?}");
                picked += 1;
            }
            // A plugin this program lacks: the program refuses the file, unless a plugin it does
            // carry binds the file at a lower priority (a picture named `.md` is the `image`
            // plugin's by content once the `markdown` plugin, which the name picks, is absent).
            PreviewKind::Plugin { id, .. } => match &got {
                Err(AppError::NotHandled { .. }) => {}
                Ok(g) if own.contains(g) => {
                    assert!(
                        LOWER_PRIORITY.contains(&(
                            name,
                            f.file_name().unwrap().to_str().unwrap(),
                            g.as_str()
                        )),
                        "{ctx}: konoma picks `{id}`, the program picks `{g}`"
                    );
                    picked += 1;
                }
                other => panic!("{ctx}: konoma picks `{id}`, which this program lacks: {other:?}"),
            },
            PreviewKind::Text(_) => match got {
                Err(AppError::NotHandled { why, .. }) => {
                    assert!(why.contains("reads as text"), "{ctx}: {why}")
                }
                other => panic!("{ctx}: konoma shows text, got {other:?}"),
            },
            _ => match got {
                Err(AppError::NotHandled { why, .. }) => {
                    assert!(!why.contains("reads as text"), "{ctx}: {why}")
                }
                other => panic!("{ctx}: konoma can not preview it, got {other:?}"),
            },
        }
    }
    picked
}

#[test]
fn every_program_picks_what_konoma_picks_over_the_corpus() {
    let dir = unique_tmp("program_parity");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let files = corpus(dir.as_path());
    // Real checks, not skipped ones: the corpus is wide and each program picks a plugin for at
    // least its floor of files.
    assert!(files.len() >= 60, "{}", files.len());
    let floors: &[(&str, usize)] = &[
        ("konoma-code", 14),
        ("konoma-markdown", 12),
        // binds only embedded blocks, so it takes no file
        ("konoma-math", 0),
        ("konoma-mermaid", 5),
        ("konoma-office", 9),
        ("konoma-table", 5),
        // The pictures, videos, SVG and PDF of samples/ and the ones made here (with no
        // extension or the wrong one).
        ("konoma-media", 18),
    ];
    for (name, make) in programs() {
        let picked = check_program(name, make, &files);
        let floor = floors.iter().find(|(n, _)| *n == name).unwrap().1;
        assert!(picked >= floor, "{name} picked {picked} < {floor}");
    }
}

#[test]
fn the_media_program_picks_pictures_and_videos_by_content_not_name() {
    let dir = unique_tmp("program_parity_media");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let files = corpus(dir.as_path());
    let by_name = |n: &str| files.iter().find(|f| f.file_name().unwrap() == n).unwrap();
    for (name, want) in [
        ("sample.png", "image"),
        ("sample.jpg", "image"),
        ("sample.gif", "image"),
        ("sample.mp4", "video"),
        ("png_noext", "image"),
        ("png_as.txt", "image"),
        ("png_as.dat", "image"),
        ("jpg_noext", "image"),
        ("jpg_as.bin", "image"),
        ("gif_noext", "image"),
        ("gif_as.txt", "image"),
        ("mp4_noext", "video"),
        ("mp4_as.dat", "video"),
    ] {
        let got = pick_plugin(arcs(konoma_media::program), by_name(name)).unwrap();
        assert_eq!(got, want, "{name}");
    }
    // A picture whose name says Markdown is still the picture (konoma-media lacks the Markdown
    // plugin that the name would pick first).
    assert_eq!(
        pick_plugin(arcs(konoma_media::program), by_name("png_as.md")).unwrap(),
        "image"
    );
    // Content that is not a picture is not taken because of its name.
    for name in ["text_as.png", "text_as.mp4"] {
        let got = pick_plugin(arcs(konoma_media::program), by_name(name));
        assert!(
            !matches!(got.as_deref(), Ok("image") | Ok("video")),
            "{name}: {got:?}"
        );
    }
}

#[test]
fn every_program_is_in_the_list() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates");
    let mut found = Vec::new();
    for e in std::fs::read_dir(crates).unwrap() {
        let dir = e.unwrap().path();
        let main = dir.join("src/main.rs");
        if let Ok(src) = std::fs::read_to_string(&main) {
            if src.contains("export!(@program") {
                found.push(dir.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    let mut listed: Vec<String> = programs().iter().map(|(n, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(found, listed);
}
