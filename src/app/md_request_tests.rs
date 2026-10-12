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
    app.md_image_cache
        .insert(path.to_path_buf(), MdImgEntry::default());
    let old = app.begin_md_request(path);
    app.md_image_cache.remove(path);
    app.md_image_cache
        .insert(path.to_path_buf(), MdImgEntry::default());
    let new = app.begin_md_request(path);
    assert_ne!(old, new, "every request gets its own id");
    (old, new)
}

#[test]
fn request_ids_are_unique_and_increasing_and_the_entry_keeps_the_latest() {
    let (mut app, _dir) = app_in("konoma_req_ids");
    let p = PathBuf::from("/x/a.png");
    app.md_image_cache.insert(p.clone(), MdImgEntry::default());
    let a = app.begin_md_request(&p);
    let b = app.begin_md_request(&p);
    assert!(a >= 1 && b > a);
    assert_eq!(app.md_image_cache[&p].request, b);
    assert_eq!(
        app.md_image_cache[&p].born, a,
        "born is the first request, kept for life"
    );
    // An entry that is not in the cache still consumes an id and changes nothing.
    let c = app.begin_md_request(Path::new("/x/none.png"));
    assert!(c > b);
    assert!(!app.md_image_cache.contains_key(Path::new("/x/none.png")));
}

#[test]
fn an_old_cancelled_result_does_not_forget_the_newer_entry() {
    let (mut app, _dir) = app_in("konoma_req_old_cancel");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    let redraw = app.apply_md_image(result(&p, old, Err(ImageFailure::Cancelled), false));
    assert!(!redraw, "a stale result asks for nothing");
    assert!(
        app.md_image_cache.contains_key(&p),
        "the newer entry survives"
    );
    // ...and the newer request's own result applies.
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert!(app.md_image_cache[&p].decoded.is_some());
}

#[test]
fn an_old_cancelled_result_arriving_after_the_new_success_changes_nothing() {
    let (mut app, _dir) = app_in("konoma_req_old_cancel_late");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert!(!app.apply_md_image(result(&p, old, Err(ImageFailure::Cancelled), false)));
    assert!(
        app.md_image_cache[&p].decoded.is_some(),
        "the landed picture stays"
    );
}

#[test]
fn an_old_success_does_not_fill_the_newer_entry() {
    let (mut app, _dir) = app_in("konoma_req_old_success");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, old, Ok(()), false)));
    let e = &app.md_image_cache[&p];
    assert!(
        e.decoded.is_none() && e.layout_px.is_none(),
        "still waiting for its own decode"
    );
    assert!(app.apply_md_image(result(&p, new, Ok(()), false)));
    assert_eq!(app.md_image_cache[&p].layout_px, Some((8, 4)));
}

#[test]
fn an_old_failure_does_not_mark_the_newer_entry_failed() {
    let (mut app, _dir) = app_in("konoma_req_old_fail");
    let p = PathBuf::from("/x/a.png");
    let (old, _new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, old, Err(ImageFailure::Corrupt), false)));
    assert!(!app.md_image_cache[&p].failed);
}

#[test]
fn an_old_reraster_does_not_replace_pixels_or_clear_the_newer_in_flight_flag() {
    let (mut app, _dir) = app_in("konoma_req_old_reraster");
    let p = PathBuf::from("/x/a.png");
    app.md_image_cache.insert(
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
    let e = &app.md_image_cache[&p];
    assert!(e.reraster_inflight, "the newer re-raster is still running");
    assert_eq!(
        e.decoded.as_ref().map(|d| d.width()),
        Some(2),
        "pixels untouched"
    );
    assert!(app.apply_md_image(result(&p, new, Ok(()), true)));
    let e = &app.md_image_cache[&p];
    assert!(!e.reraster_inflight);
    assert_eq!(e.decoded.as_ref().map(|d| d.width()), Some(8));
}

#[test]
fn the_current_requests_cancelled_result_still_forgets_the_entry() {
    let (mut app, _dir) = app_in("konoma_req_current_cancel");
    let p = PathBuf::from("/x/a.png");
    let (_old, new) = two_lives(&mut app, &p);
    assert!(!app.apply_md_image(result(&p, new, Err(ImageFailure::Cancelled), false)));
    assert!(!app.md_image_cache.contains_key(&p));
}

#[test]
fn a_result_for_a_missing_entry_is_still_ignored() {
    let (mut app, _dir) = app_in("konoma_req_gone");
    assert!(!app.apply_md_image(result(Path::new("/x/gone.png"), 7, Ok(()), false)));
    assert!(app.md_image_cache.is_empty());
}

#[test]
fn an_encode_of_a_dropped_entry_is_not_applied_to_the_newer_one() {
    let (mut app, _dir) = app_in("konoma_req_encode");
    let p = PathBuf::from("/x/a.png");
    let (old, new) = two_lives(&mut app, &p);
    app.md_image_cache.get_mut(&p).unwrap().enc_inflight = true;
    let key = MdEncodeKey::Full { cols: 4, rows: 2 };
    let stale = MdEncodeResult {
        path: p.clone(),
        key,
        image: None,
        born: old,
    };
    assert!(!app.apply_md_encode(stale));
    let e = &app.md_image_cache[&p];
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
    assert!(!app.md_image_cache[&p].enc_inflight);
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
    assert!(app.md_image_cache.contains_key(&png));
    // Leaving the document: the entry goes (its decode result is still on its way).
    app.md_image_cache.remove(&png);
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let t = std::time::Instant::now();
    while app
        .md_image_cache
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
    assert!(app.md_image_cache.contains_key(&png));
}
