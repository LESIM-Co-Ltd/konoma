//! A result of an earlier request for a path must never touch the entry of a newer request for
//! the same path (the entry was dropped and the picture asked for again): the race that made a
//! cancelled first decode forget the entry of the decode that replaced it.

use super::*;
use crate::config::Config;
use crate::preview::image::ImageFailure;
use crate::test_support::unique_tmp;

fn app_in(name: &str) -> (App, crate::test_support::TmpDir) {
    let dir = unique_tmp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    (app, dir)
}

fn result(
    path: &Path,
    request: u64,
    image: Result<(), ImageFailure>,
    reraster: bool,
) -> MdImageResult {
    MdImageResult {
        path: path.to_path_buf(),
        image: image
            .map(|()| image::DynamicImage::new_rgba8(8, 4))
            .map_err(|f| f.code().to_string()),
        svg: None,
        reraster,
        frames: None,
        request,
    }
}

/// Two lives of one path: the first request is started, the entry dropped (the preview moved on),
/// the picture asked for again. Returns the ids of the old and the new request.
fn two_lives(app: &mut App, path: &Path) -> (u64, u64) {
    app.md_images
        .cache
        .insert(path.to_path_buf(), MdImgEntry::default());
    let old = app.begin_md_request(path);
    app.md_images.cache.remove(path);
    app.md_images
        .cache
        .insert(path.to_path_buf(), MdImgEntry::default());
    let new = app.begin_md_request(path);
    assert_ne!(old, new, "every request gets its own id");
    (old, new)
}

#[test]
fn request_ids_are_unique_and_increasing_and_the_entry_keeps_the_latest() {
    let (mut app, _dir) = app_in("konoma_req_ids");
    let p = PathBuf::from("/x/a.png");
    app.md_images.cache.insert(p.clone(), MdImgEntry::default());
    let a = app.begin_md_request(&p);
    let b = app.begin_md_request(&p);
    assert!(a >= 1 && b > a);
    assert_eq!(app.md_images.cache[&p].request, b);
    assert_eq!(
        app.md_images.cache[&p].born, a,
        "born is the first request, kept for life"
    );
    // An entry that is not in the cache still consumes an id and changes nothing.
    let c = app.begin_md_request(Path::new("/x/none.png"));
    assert!(c > b);
    assert!(!app.md_images.cache.contains_key(Path::new("/x/none.png")));
}

#[test]
fn an_old_cancelled_result_does_not_forget_the_newer_entry() {
    let (mut app, _dir) = app_in("konoma_req_old_cancel");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    let redraw = app.apply_md_image(result(&p, old, Err(ImageFailure::Cancelled), false));
    assert!(!redraw, "a stale result asks for nothing");
    assert!(
        app.md_images.cache.contains_key(&p),
        "the newer entry survives"
    );
    // ...and the newer request's own result applies.
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert!(app.md_images.cache[&p].decoded.is_some());
}

#[test]
fn an_old_cancelled_result_arriving_after_the_new_success_changes_nothing() {
    let (mut app, _dir) = app_in("konoma_req_old_cancel_late");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert!(!app.apply_md_image(result(&p, old, Err(ImageFailure::Cancelled), false)));
    assert!(
        app.md_images.cache[&p].decoded.is_some(),
        "the landed picture stays"
    );
}

#[test]
fn an_old_success_does_not_fill_the_newer_entry() {
    let (mut app, _dir) = app_in("konoma_req_old_success");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, old, Ok(()), false)));
    let e = &app.md_images.cache[&p];
    assert!(
        e.decoded.is_none() && e.layout_px.is_none(),
        "still waiting for its own decode"
    );
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert_eq!(app.md_images.cache[&p].layout_px, Some((8, 4)));
}

#[test]
fn an_old_failure_does_not_mark_the_newer_entry_failed() {
    let (mut app, _dir) = app_in("konoma_req_old_fail");
    let p = PathBuf::from("/x/a.png");
    let (old, _new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, old, Err(ImageFailure::Corrupt), false)));
    assert!(!app.md_images.cache[&p].failed);
}

#[test]
fn an_old_reraster_does_not_replace_pixels_or_clear_the_newer_in_flight_flag() {
    let (mut app, _dir) = app_in("konoma_req_old_reraster");
    let p = PathBuf::from("/x/a.png");
    app.md_images.cache.insert(
        p.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::new_rgba8(2, 2))),
            reraster_inflight: true,
            ..Default::default()
        },
    );
    let old = app.begin_md_request(&p);
    // A newer re-raster superseded it.
    let new = app.begin_md_request(&p);
    assert!(!app.apply_md_image(result(&p, old, Ok(()), true)));
    let e = &app.md_images.cache[&p];
    assert!(e.reraster_inflight, "the newer re-raster is still running");
    assert_eq!(
        e.decoded.as_ref().map(|d| d.width()),
        Some(2),
        "pixels untouched"
    );
    assert!(app.apply_md_image(result(&p, new, Ok(()), true)));
    let e = &app.md_images.cache[&p];
    assert!(!e.reraster_inflight);
    assert_eq!(e.decoded.as_ref().map(|d| d.width()), Some(8));
}

#[test]
fn the_current_requests_cancelled_result_still_forgets_the_entry() {
    let (mut app, _dir) = app_in("konoma_req_current_cancel");
    let p = PathBuf::from("/x/a.png");
    let (_old, new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, new, Err(ImageFailure::Cancelled), false)));
    assert!(!app.md_images.cache.contains_key(&p));
}

#[test]
fn a_result_for_a_missing_entry_is_still_ignored() {
    let (mut app, _dir) = app_in("konoma_req_gone");
    assert!(!app.apply_md_image(result(Path::new("/x/gone.png"), 7, Ok(()), false)));
    assert!(app.md_images.cache.is_empty());
}

#[test]
fn an_encode_of_a_dropped_entry_is_not_applied_to_the_newer_one() {
    let (mut app, _dir) = app_in("konoma_req_encode");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    app.md_images.cache.get_mut(&p).unwrap().enc_inflight = true;
    let key = MdEncodeKey::Full { cols: 4, rows: 2 };
    let stale = MdEncodeResult {
        path: p.clone(),
        key,
        image: None,
        born: old,
    };
    assert!(!app.apply_md_encode(stale));
    let e = &app.md_images.cache[&p];
    assert!(e.enc_inflight, "the newer encode is still owed");
    assert!(
        !e.failed,
        "a stale failed encode does not degrade the newer entry"
    );
    let current = MdEncodeResult {
        path: p.clone(),
        key,
        image: None,
        born: new,
    };
    assert!(app.apply_md_encode(current));
    assert!(!app.md_images.cache[&p].enc_inflight);
}

/// The race with the real threads: open, leave (the entry goes), reopen. Whatever order the
/// results arrive in, the entry of the second request ends up decoded.
#[test]
fn reasking_for_a_picture_after_its_entry_was_dropped_ends_decoded() {
    let dir = unique_tmp("konoma_req_real_threads");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("p.png");
    image::RgbaImage::from_pixel(8, 4, image::Rgba([1, 2, 3, 255]))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_image_loader(tx);
    let url = png.to_string_lossy().to_string();
    app.tab.mode = Mode::Preview;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    assert!(app.md_images.cache.contains_key(&png));
    // Leaving the document: the entry goes (its decode result is still on its way).
    app.md_images.cache.remove(&png);
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let t = std::time::Instant::now();
    while app
        .md_images
        .cache
        .get(&png)
        .is_none_or(|e| e.decoded.is_none())
    {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "the new decode never landed"
        );
        if let Ok(res) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
            app.apply_md_image(res);
        }
        // A frame later the picture is asked for again if its entry was forgotten.
        app.ensure_md_image(&url, 10, 4, 0, 4);
    }
    assert!(app.md_images.cache.contains_key(&png));
}

// ---- An encode cut from pixels that were replaced while it ran ------------------------------------
//
// A diagram's sharpening re-raster can land while an encode of the old raster is on the worker. The
// old encode then arrives after `mark_stale`; it used to clear the slot's `stale` flag, so the band
// stayed cut from the old density for good (v0.31.0 as well: only the timing decided which picture
// a partly visible figure at the screen edge ended up with).

fn solid_rgb(w: u32, h: u32, rgb: [u8; 3]) -> image::DynamicImage {
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        w,
        h,
        image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
    ))
}

/// A real encode worker (halfblocks, like a terminal without graphics), driven one request at a
/// time by the test so the order in which results land is the test's.
struct Worker {
    jobs: std::sync::mpsc::Sender<MdEncodeRequest>,
    results: std::sync::mpsc::Receiver<MdEncodeResult>,
}

impl Worker {
    fn start() -> Self {
        let (jobs, job_rx) = std::sync::mpsc::channel();
        let (res_tx, results) = std::sync::mpsc::channel();
        let picker = ratatui_image::picker::Picker::halfblocks();
        std::thread::spawn(move || md_encode_worker(picker, job_rx, res_tx));
        Self { jobs, results }
    }

    fn encode(&self, req: MdEncodeRequest) -> MdEncodeResult {
        self.jobs.send(req).unwrap();
        self.results
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the worker answers")
    }
}

/// The RGB foreground colours the picture `url` draws at this position into a `cols` x `vis_rows`
/// buffer (halfblocks paint the picture as coloured cells).
fn drawn_colours(
    app: &App,
    url: &str,
    cols: u16,
    full_rows: u16,
    row_off: u16,
    vis_rows: u16,
) -> Colours {
    let proto = app
        .md_image_proto(url, cols, full_rows, row_off, vis_rows)
        .expect("a picture is drawable at this position");
    let area = ratatui::layout::Rect::new(0, 0, cols, vis_rows);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    proto.render(area, &mut buf);
    buf.content()
        .iter()
        .filter_map(|c| match c.style().fg {
            Some(ratatui::style::Color::Rgb(r, g, b)) => Some((r, g, b)),
            _ => None,
        })
        .collect()
}

/// The foreground colours of the cells a picture drew.
type Colours = Vec<(u8, u8, u8)>;

/// Set up `url` with a red raster, ask for the encode at `pos`, land a blue re-raster while that
/// encode is still on the worker, then let the old encode land. Returns what the screen holds then
/// and what it holds once the renderer has been through the following passes.
fn replaced_while_encoding(name: &str, url: &str, pos: (u16, u16, u16, u16)) -> (Colours, Colours) {
    let (cols, full_rows, row_off, vis_rows) = pos;
    let (mut app, _dir) = app_in(name);
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.tab.mode = Mode::Preview;
    let (enc_tx, enc_rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(enc_tx);
    let worker = Worker::start();
    let p = PathBuf::from(url);
    app.md_images.cache.insert(
        p.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(solid_rgb(200, 100, [250, 0, 0]))),
            layout_px: Some((200, 100)),
            ..Default::default()
        },
    );
    let request = app.begin_md_request(&p);

    app.ensure_md_image(url, cols, full_rows, row_off, vis_rows);
    let first = enc_rx
        .try_recv()
        .expect("the first pass asks for an encode");

    // The sharper raster lands while that encode is on the worker.
    app.md_images.cache.get_mut(&p).unwrap().reraster_inflight = true;
    assert!(app.apply_md_image(MdImageResult {
        path: p.clone(),
        image: Ok(solid_rgb(400, 200, [0, 0, 250])),
        svg: None,
        reraster: true,
        frames: None,
        request,
    }));
    // ...and the old encode lands after it.
    assert!(app.apply_md_encode(worker.encode(first)));
    let shown_meanwhile = drawn_colours(&app, url, cols, full_rows, row_off, vis_rows);

    // The following passes of the renderer.
    for _ in 0..4 {
        app.ensure_md_image(url, cols, full_rows, row_off, vis_rows);
        match enc_rx.try_recv() {
            Ok(req) => {
                assert!(app.apply_md_encode(worker.encode(req)));
            }
            Err(_) => break,
        }
    }
    // Converged: nothing more is asked for.
    app.ensure_md_image(url, cols, full_rows, row_off, vis_rows);
    assert!(
        enc_rx.try_recv().is_err(),
        "the renderer asks for nothing more once the picture is final"
    );
    let settled = drawn_colours(&app, url, cols, full_rows, row_off, vis_rows);
    (shown_meanwhile, settled)
}

fn is_red(c: &(u8, u8, u8)) -> bool {
    c.0 > 200 && c.1 < 40 && c.2 < 40
}

fn is_blue(c: &(u8, u8, u8)) -> bool {
    c.2 > 200 && c.0 < 40 && c.1 < 40
}

/// The positions a picture is drawn at: one row of a band showing only the top of the picture (a
/// figure peeking in at the bottom edge), one row at its bottom (peeking in at the top edge), and
/// the whole picture.
const EDGE_POSITIONS: [(u16, u16, u16, u16); 3] = [(20, 10, 0, 1), (20, 10, 9, 1), (20, 10, 0, 10)];

#[test]
fn an_encode_of_replaced_pixels_is_redone_for_a_figure_at_either_screen_edge_and_whole() {
    for (i, pos) in EDGE_POSITIONS.into_iter().enumerate() {
        let url = crate::preview::markdown::mermaid_fence_url("graph TD\nA-->B");
        let (meanwhile, settled) =
            replaced_while_encoding(&format!("konoma_outdated_fence_{i}"), &url, pos);
        assert!(
            !meanwhile.is_empty() && meanwhile.iter().all(is_red),
            "{pos:?}: until the new encode lands the old picture keeps drawing (no blank)"
        );
        assert!(
            !settled.is_empty() && settled.iter().all(is_blue),
            "{pos:?}: it ends up cut from the sharpened raster: {settled:?}"
        );
    }
}

#[test]
fn an_encode_of_replaced_pixels_is_redone_for_a_formula_too() {
    for (i, pos) in EDGE_POSITIONS.into_iter().enumerate() {
        let url = crate::preview::markdown::math_url("x^2", true);
        let (_, settled) = replaced_while_encoding(&format!("konoma_outdated_math_{i}"), &url, pos);
        assert!(
            !settled.is_empty() && settled.iter().all(is_blue),
            "{pos:?}: {settled:?}"
        );
    }
}

/// The pixels of an entry that nothing replaced are final after one encode: the flag is only set
/// by a replacement that happens while an encode is running.
#[test]
fn an_encode_of_unchanged_pixels_is_final_and_a_replacement_between_encodes_leaves_no_mark() {
    let (mut app, _dir) = app_in("konoma_outdated_none");
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.tab.mode = Mode::Preview;
    let (enc_tx, enc_rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(enc_tx);
    let worker = Worker::start();
    let url = crate::preview::markdown::mermaid_fence_url("graph TD\nA-->B");
    let p = PathBuf::from(&url);
    app.md_images.cache.insert(
        p.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(solid_rgb(200, 100, [250, 0, 0]))),
            layout_px: Some((200, 100)),
            ..Default::default()
        },
    );
    app.begin_md_request(&p);
    app.ensure_md_image(&url, 20, 10, 0, 1);
    assert!(app.apply_md_encode(worker.encode(enc_rx.try_recv().unwrap())));
    app.ensure_md_image(&url, 20, 10, 0, 1);
    assert!(
        enc_rx.try_recv().is_err(),
        "an encode of the current pixels is final"
    );
    // Replaced with no encode running: nothing to mark, the next pass encodes the stale slot once.
    app.md_images.cache.get_mut(&p).unwrap().mark_stale();
    assert!(!app.md_images.cache[&p].outdated_encode_in_flight);
    app.ensure_md_image(&url, 20, 10, 0, 1);
    let again = enc_rx.try_recv().expect("a stale slot is encoded again");
    assert!(app.apply_md_encode(worker.encode(again)));
    app.ensure_md_image(&url, 20, 10, 0, 1);
    assert!(enc_rx.try_recv().is_err(), "and then it is final");
}
