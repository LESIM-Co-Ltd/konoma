//! A still picture in the konoma host and in the standalone app (`konoma-plugin` feature `app`) is
//! drawn by the same shared code. Given the same file and the same keys, the two must show the
//! same picture cells, the same zoom, the same footer hints, the same help rows for the keys both
//! have, and the same words when the picture cannot be shown. (The host has more: pages, the
//! outline, the editor, bookmarks, and so on.)
//!
//! Everything runs on a `TestBackend` with a halfblocks picker; the host's resize answers are
//! applied by the test itself, the app is driven only when it is idle, so nothing depends on timing.

use super::table_parity::{help_lines, hints, presses, rows_of, same_row, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, open_file, run_with_pictures, AppCx, AppEvent, AppKeys, EventSource, Next,
    Pictures,
};
use konoma_plugin::prelude::*;
use konoma_plugin::view::{ImageData, ImageView};
use konoma_plugin::TermCaps;
use ratatui::style::Color as RColor;
use ratatui_image::picker::Picker;
use std::sync::Arc;
use std::time::Duration;

/// A plugin that shows every `*.png` as a still picture of the file itself.
struct PngPlugin;

impl Plugin for PngPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("png-parity", "0.0.1")
            .bind(Bind::files(["*.png"]))
            .shows(ViewKind::Image)
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::new(
                konoma_plugin::session::FailureKind::Unsupported,
                "a file only",
            ));
        };
        Ok(Box::new(PngSession(path)))
    }
}

struct PngSession(std::path::PathBuf);

impl Session for PngSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Ok(View::Image(ImageView::new(ImageData::File(self.0.clone()))))
    }
}

/// The keys one after the other, each only when the app is idle (nothing in flight: no decode,
/// no resize, no spinner), then the end. A wait is answered at once as a timeout.
pub(super) struct WhenIdle {
    pub(super) keys: std::collections::VecDeque<AppEvent>,
    /// How many turns the loop took (a lower bound for the "it really ran" checks).
    pub(super) turns: usize,
}

impl EventSource for WhenIdle {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        self.turns += 1;
        match wait {
            Some(w) => {
                std::thread::sleep(w.min(Duration::from_millis(2)));
                Next::Timeout
            }
            None => match self.keys.pop_front() {
                Some(k) => Next::Event(k),
                None => Next::End,
            },
        }
    }
}

/// A screen: its rows, and the cells inside the picture's frame (symbol and both colors).
pub(super) struct Shot {
    pub(super) rows: Vec<String>,
    pub(super) cells: Vec<(String, RColor, RColor)>,
}

pub(super) fn shot(term: &Terminal<TestBackend>) -> Shot {
    let rows = rows_of(term);
    let buf = term.backend().buffer();
    let top = rows.iter().position(|l| l.starts_with('┌'));
    let bottom = rows.iter().rposition(|l| l.starts_with('└'));
    let mut cells = Vec::new();
    if let (Some(t), Some(b)) = (top, bottom) {
        for y in t + 1..b {
            for x in 1..buf.area.width - 1 {
                let c = &buf[(x, y as u16)];
                cells.push((c.symbol().to_string(), c.fg, c.bg));
            }
        }
    }
    Shot { rows, cells }
}

/// What the app shows for `path` after the presses (`pictures` false: no picture backend).
fn app_shot(path: &std::path::Path, h: u16, spec: &str, pictures: bool) -> Shot {
    let mut opened =
        open_file(vec![Arc::new(PngPlugin)], path, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(W, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
    };
    let keys = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = WhenIdle { keys, turns: 0 };
    let pics =
        pictures.then(|| Pictures::new(Picker::halfblocks(), "png-parity", &opened.path, 800));
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        pics,
    )
    .unwrap();
    assert!(
        events.keys.is_empty(),
        "the app ended before the keys ran out: {spec}"
    );
    assert!(events.turns > 0);
    shot(&term)
}

/// What the host shows for `file` after the presses (`picker` false: no picture backend).
fn host_shot(root: &std::path::Path, file: &str, h: u16, spec: &str, picker: bool) -> Shot {
    let mut s = Sim::with_config_sized(root, cfg_en(), W, h);
    let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel();
    if picker {
        s = s.with_picker().with_media();
        s.app.attach_image_backend(Picker::halfblocks(), req_tx);
    }
    s.select(file);
    s.enter();
    // The decode runs on the loader's thread and its answer is awaited; then every resize the
    // draw asks for is answered on this thread until a draw asks for no more.
    if picker {
        let rx = s.media_rx.as_ref().expect("with_media");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap_or_else(|e| panic!("{file}: no answer from the media loader: {e:?}"));
        assert!(s.app.apply_media(res), "{file}: the answer is current");
    }
    let settle = |s: &mut Sim, req_rx: &mut tokio::sync::mpsc::UnboundedReceiver<_>| {
        s.draw();
        while let Ok(req) = req_rx.try_recv() {
            let req: ratatui_image::thread::ResizeRequest = req;
            s.app.apply_image_resize(req.resize_encode());
            s.draw();
        }
    };
    settle(&mut s, &mut req_rx);
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended before the keys ran out: {spec}");
        settle(&mut s, &mut req_rx);
    }
    shot(&s.term)
}

/// The zoom the host's status row shows (`x1.25`), or None.
pub(super) fn host_zoom(rows: &[String]) -> Option<String> {
    rows[0]
        .split_whitespace()
        .find(|w| w.starts_with('x') && w[1..].parse::<f64>().is_ok())
        .map(str::to_string)
}

/// The zoom the app's footer shows.
pub(super) fn app_zoom(rows: &[String]) -> Option<String> {
    hints(rows.last()?)
        .into_iter()
        .find(|w| w.starts_with('x') && w[1..].parse::<f64>().is_ok())
}

fn fixtures() -> crate::test_support::TmpDir {
    let dir = sandbox("image_parity");
    const COLORS: [[u8; 3]; 4] = [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 0]];
    let bands = |w: u32, h: u32, name: &str| {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb(COLORS[(x / 40 + y / 40) as usize % COLORS.len()])
        });
        image::DynamicImage::ImageRgb8(img)
            .save(dir.join(name))
            .unwrap();
    };
    bands(320, 320, "pic.png");
    bands(400, 100, "wide.png");
    bands(100, 400, "tall.png");
    bands(16, 16, "tiny.png");
    // A picture cut in half, and a header that announces 40,000 x 40,000 pixels.
    let bytes = std::fs::read(dir.join("pic.png")).unwrap();
    std::fs::write(dir.join("cut.png"), &bytes[..bytes.len() / 2]).unwrap();
    std::fs::write(
        dir.join("junk.png"),
        b"\x89PNG\r\n\x1a\nthis is not a picture",
    )
    .unwrap();
    std::fs::write(dir.join("huge.png"), huge_png(40_000, 40_000)).unwrap();
    dir
}

/// A real 1-pixel PNG with its size fields patched (and the checksum redone), so the header
/// announces `w` x `h` pixels.
fn huge_png(w: u32, h: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&[1, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    out[16..20].copy_from_slice(&w.to_be_bytes());
    out[20..24].copy_from_slice(&h.to_be_bytes());
    let mut crc = !0u32;
    for &byte in &out[12..29] {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    out[29..33].copy_from_slice(&(!crc).to_be_bytes());
    out
}

/// Keys over a picture: the zoom both ways and back to fit, then the pans at fit and zoomed
/// (the clipped axis moves, the other does not), up to the zoom where both axes are clipped.
const SPECS: &[&str] = &[
    "",
    "+",
    "+ +",
    "+ + +",
    "+ + + +",
    "+ + + + +",
    "-",
    "- -",
    "+ -",
    "+ + -",
    "+ + - -",
    "+ + - - -",
    "+ =",
    "+ + + =",
    "=",
    "= +",
    "+ = +",
    "h",
    "j",
    "k",
    "l",
    "h j k l",
    "<Left> <Down> <Up> <Right>",
    "+ h",
    "+ l",
    "+ j",
    "+ k",
    "+ + j",
    "+ + k",
    "+ + + k",
    "+ + + l",
    "+ + + h",
    "+ + + + l l l",
    "+ + + + + j j j j l l l l",
];

/// What only the host's footer says (the files, the editor, the tabs, the pages, `q` going
/// back), spelled out in full: a word the host adds that is not listed here fails the comparison.
pub(super) const HOST_ONLY: &[&str] = &[
    ",:view",
    "[/]:tab",
    "C-n/C-p:next/prev file",
    "e:edit",
    "o:open in app",
    "q:back",
];

#[test]
fn the_picture_the_zoom_and_the_footer_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let (mut checked, mut moved) = (0, 0);
    let mut seen_host_only = std::collections::BTreeSet::new();
    let mut zooms = std::collections::BTreeSet::new();
    for file in ["pic.png", "wide.png", "tall.png"] {
        let at_fit = host_shot(&root, file, HOST_H, "", true);
        for spec in SPECS {
            let what = format!("{file} [{spec}]");
            let h = host_shot(&root, file, HOST_H, spec, true);
            let a = app_shot(&root.join(file), APP_H, spec, true);
            assert!(h.cells.len() > 400, "{what}: the host frame is there");
            assert_eq!(
                h.cells.len(),
                a.cells.len(),
                "{what}: the frames differ in size"
            );
            // The same picture cells, colors included.
            if h.cells != a.cells {
                let first = h
                    .cells
                    .iter()
                    .zip(&a.cells)
                    .position(|(x, y)| x != y)
                    .unwrap();
                panic!(
                    "{what}: the picture differs at cell {first}: host {:?} app {:?}\nhost:\n{}\napp:\n{}",
                    h.cells[first],
                    a.cells[first],
                    h.rows.join("\n"),
                    a.rows.join("\n")
                );
            }
            // The same zoom.
            let (hz, az) = (host_zoom(&h.rows), app_zoom(&a.rows));
            assert!(
                hz.is_some(),
                "{what}: the host shows a zoom: {:?}",
                h.rows[0]
            );
            assert_eq!(hz, az, "{what}: the zoom differs");
            zooms.insert(hz.unwrap());
            // The footer.
            let (hh, ah) = (
                hints(h.rows.last().expect("footer")),
                hints(a.rows.last().expect("footer")),
            );
            // The app's tail is `x1.25` and the title; the host has the zoom in its status row.
            let ah: Vec<String> = ah[..ah.len() - 2].to_vec();
            for t in hh.iter().filter(|t| !ah.contains(t)) {
                seen_host_only.insert(t.clone());
                assert!(
                    HOST_ONLY.contains(&t.as_str()),
                    "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            for t in ah.iter().filter(|t| !hh.contains(t)) {
                assert_eq!(
                    t, "q:quit",
                    "{what}: the app footer has `{t}` that the host lacks"
                );
            }
            checked += 1;
            moved += usize::from(h.cells != at_fit.cells);
        }
    }
    assert!(checked >= 3 * 30, "{checked}");
    // The keys moved the picture (a spec that never changed anything would prove nothing), and
    // took it through many zooms.
    assert!(moved >= 50, "{moved}");
    assert!(zooms.len() >= 6, "{zooms:?}");
    for p in HOST_ONLY {
        assert!(seen_host_only.contains(*p), "HOST_ONLY `{p}` never occurs");
    }
}

/// The failure line of a screen: the text inside the frame, if any.
fn failure_of(rows: &[String]) -> Option<String> {
    rows.iter()
        .find(|r| r.contains("[image]") || r.contains("cannot display image"))
        .map(|r| {
            r.trim_start_matches('│')
                .trim_end_matches('│')
                .trim()
                .to_string()
        })
}

#[test]
fn a_picture_that_cannot_be_shown_says_the_same_as_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let mut compared = 0;
    let mut said = std::collections::BTreeSet::new();
    for file in ["cut.png", "junk.png", "huge.png"] {
        let h = host_shot(&root, file, HOST_H, "", true);
        let a = app_shot(&root.join(file), APP_H, "", true);
        let (hf, af) = (failure_of(&h.rows), failure_of(&a.rows));
        assert!(
            hf.is_some(),
            "{file}: the host says why\n{}",
            h.rows.join("\n")
        );
        assert_eq!(hf, af, "{file}: the wording differs");
        said.insert(hf.unwrap());
        compared += 1;
    }
    // No picture backend at all: the host and the app name the terminal.
    for file in ["pic.png", "cut.png"] {
        let h = host_shot(&root, file, HOST_H, "", false);
        let a = app_shot(&root.join(file), APP_H, "", false);
        let (hf, af) = (failure_of(&h.rows), failure_of(&a.rows));
        assert!(
            hf.is_some(),
            "{file}: the host says why\n{}",
            h.rows.join("\n")
        );
        assert_eq!(hf, af, "{file} without a backend: the wording differs");
        said.insert(hf.unwrap());
        compared += 1;
    }
    assert!(compared >= 5, "{compared}");
    // Damaged, too large and the terminal sentence: the wording is not one fixed line.
    assert!(said.len() >= 3, "{said:?}");
}

/// The rows of the image section of a help: from its heading to the blank row.
pub(super) fn image_section(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| l == "Preview: image")
        .unwrap_or_else(|| panic!("the image section in {:#?}", &lines[..lines.len().min(12)]));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.is_empty() && !l.starts_with('│'))
        .cloned()
        .collect()
}

/// What only the host's image help lists (the files, bookmarks, refresh), spelled out in full.
pub(super) const HOST_ONLY_ROWS: &[&str] = &["Ctrl-n / Ctrl-p", "m / '", "r refresh"];

#[test]
fn the_image_help_rows_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let (mut compared, mut host_only) = (0, std::collections::BTreeSet::new());
    for file in ["pic.png", "wide.png", "tiny.png"] {
        for spec in [
            "?",
            "+ ?",
            "+ + + ?",
            "+ = ?",
            "h ?",
            "- ?",
            "+ + + + + j l ?",
        ] {
            let what = format!("{file} [{spec}]");
            let h = host_shot(&root, file, 70, spec, true);
            let a = app_shot(&root.join(file), 69, spec, true);
            let (py, px) = h
                .rows
                .iter()
                .enumerate()
                .find_map(|(y, r)| {
                    r.find("┌ Help")
                        .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
                })
                .expect("the help panel");
            let hs = image_section(&help_lines(&h.rows[py + 1..], Some(px)));
            let asec = image_section(&help_lines(&a.rows, None));
            for row in &asec {
                // The app says `quit` where the host says `back to tree`; `?` and `Q` are the
                // host's own rows in other sections of its help.
                if row.starts_with("q / Esc") {
                    assert!(hs.iter().any(|r| r.starts_with("q / Esc")), "{what}: {row}");
                    continue;
                }
                if row.starts_with("? ") || row.starts_with("Q ") {
                    continue;
                }
                assert!(
                    hs.iter().any(|r| same_row(r, row)),
                    "{what}: the app's help row `{row}` is not in the host's\nhost: {hs:#?}\napp: {asec:#?}"
                );
                compared += 1;
            }
            for row in &hs {
                if row.starts_with("q / Esc") || asec.iter().any(|r| same_row(r, row)) {
                    continue;
                }
                let known = HOST_ONLY_ROWS.iter().find(|p| row.starts_with(**p));
                assert!(
                    known.is_some(),
                    "{what}: the host's help row `{row}` is not in the app's\nhost: {hs:#?}\napp: {asec:#?}"
                );
                host_only.insert(*known.unwrap());
            }
            // The order of the shared rows is the host's.
            let order = |v: &[String], other: &[String]| -> Vec<String> {
                v.iter()
                    .filter(|r| other.iter().any(|x| same_row(x, r)))
                    .map(|r| r.chars().take(8).collect::<String>())
                    .collect()
            };
            assert_eq!(
                order(&hs, &asec),
                order(&asec, &hs),
                "{what}: the shared rows are in another order"
            );
        }
    }
    assert!(compared >= 3 * 7 * 3, "{compared}");
    assert_eq!(host_only.len(), HOST_ONLY_ROWS.len(), "{host_only:?}");
}
