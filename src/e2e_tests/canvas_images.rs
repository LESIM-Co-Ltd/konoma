//! The pictures a plugin places on its canvas (`CanvasImage`), drawn by the host, stage 2.5 / T2b.
//!
//! A stand-in plugin (`Sprites`) builds its canvas from a closure over the request and the number
//! of the `view` call, so a test can place pictures relative to the real area, change them
//! between redraws and send the same picture again. The tests run the real pipeline with a kitty
//! picker (`with_media_kitty`: the worker decodes, the encode worker encodes, the buffer carries
//! the placeholder rows and the transmit escapes) and read the result off the drawn buffer.

use super::*;
use crate::plugins::test_plugins;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{CanvasImage, CanvasView, CellRect, ImageData, Layout};
use konoma_plugin::Style as PStyle;
use ratatui::style::{Color as RColor, Modifier as RModifier};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app::canvas_images::content_hash;
use crate::app::canvas_images::seen_for_tests::decoded_count;

type Build = Arc<dyn Fn(&ViewReq, u32) -> CanvasView + Send + Sync>;
// (the second argument of a build is the phase)

/// A plugin whose canvas is whatever its closure builds.
#[derive(Clone)]
struct Sprites {
    id: String,
    build: Build,
    views: Arc<AtomicUsize>,
    /// Which version of the canvas the closure builds; a test sets it before asking again.
    phase: Arc<AtomicUsize>,
    reqs: Arc<Mutex<Vec<ViewReq>>>,
}

impl Sprites {
    fn new(
        id: &str,
        build: impl Fn(&ViewReq, u32) -> CanvasView + Send + Sync + 'static,
    ) -> Sprites {
        Sprites {
            id: id.to_string(),
            build: Arc::new(build),
            views: Arc::default(),
            phase: Arc::default(),
            reqs: Arc::default(),
        }
    }
}

impl Plugin for Sprites {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.spr"]))
            .shows(ViewKind::Canvas)
            .action(
                Act::new("again")
                    .key("x")
                    .label(t!(en: "again", ja: "再描画"))
                    .help(t!(en: "Draw the canvas again", ja: "もう一度描く"))
                    .footer(1),
            )
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(SpritesSession { p: self.clone() }))
    }
}

struct SpritesSession {
    p: Sprites,
}

impl Session for SpritesSession {
    fn layout(&self) -> Layout {
        Layout::single()
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.p.views.fetch_add(1, Ordering::SeqCst);
        self.p.reqs.lock().unwrap().push(req.clone());
        let phase = self.p.phase.load(Ordering::SeqCst) as u32;
        let mut canvas = (self.p.build)(req, phase);
        // The first request comes before the area was measured (one row): a picture placed
        // relative to the area may not fit that canvas, and a canvas with a picture outside it is
        // refused. The next request has the real size.
        canvas
            .images
            .retain(|im| im.x + im.cols <= canvas.cols && im.y + im.rows <= canvas.rows);
        Ok(View::Canvas(canvas))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        match action {
            "again" => Ok(Outcome::Redraw),
            _ => Ok(Outcome::Unhandled),
        }
    }
}

// ---- helpers -----------------------------------------------------------------------------------

/// A solid RGBA picture of `w` x `h` pixels.
fn solid(w: u32, h: u32, c: [u8; 3]) -> ImageData {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        px.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    ImageData::Rgba {
        width: w,
        height: h,
        pixels: Arc::new(px),
    }
}

fn placed(id: u64, x: u16, y: u16, cols: u16, rows: u16, image: ImageData) -> CanvasImage {
    CanvasImage {
        id,
        x,
        y,
        cols,
        rows,
        image,
    }
}

/// The pixel size that fills `cols` x `rows` cells exactly for the cell size the host reported
/// (`ViewReq::cell_px`), so the encode is the whole rectangle and not a fitted part of it.
fn fill_px(req: &ViewReq, cols: u16, rows: u16) -> (u32, u32) {
    let (cw, ch) = req.cell_px.unwrap_or((10, 20));
    (cols as u32 * cw as u32, rows as u32 * ch as u32)
}

struct Opened {
    s: Sim,
    p: Sprites,
    /// Every kitty id transmitted by a frame drawn so far, in the order of the frames.
    transmits: Vec<u32>,
    _dir: crate::test_support::TmpDir,
}

/// Open `a.spr` with the plugin on a kitty terminal with the whole media pipeline.
fn open_kitty(name: &str, p: Sprites, w: u16, h: u16) -> Opened {
    open_on(name, p, w, h, |s| s.with_media_kitty())
}

fn open_on(name: &str, p: Sprites, w: u16, h: u16, attach: impl FnOnce(Sim) -> Sim) -> Opened {
    let dir = sandbox(name);
    std::fs::write(dir.join("a.spr"), b"x").unwrap();
    let marker = test_plugins::register(name, Arc::new(p.clone()));
    let cfg = test_plugins::marker_config(&marker);
    let s = Sim::with_config_sized(&canon(&dir), cfg, w, h);
    let mut s = attach(s);
    s.select("a.spr");
    s.enter();
    let mut o = Opened {
        s,
        p,
        transmits: Vec::new(),
        _dir: dir,
    };
    o.pump();
    o
}

impl Opened {
    /// Draw, record what the frame transmitted.
    fn draw(&mut self) {
        self.s.draw();
        self.transmits.extend(transmitted_kitty_ids(&self.s.term));
    }

    /// Let every background answer land (the plugin's, then the encodes) and draw after each
    /// until nothing is out. Records the transmits of every frame.
    #[track_caller]
    fn pump(&mut self) {
        for _ in 0..60 {
            if self.s.app.is_media_loading() {
                self.s.drain_plugin();
                self.transmits.extend(transmitted_kitty_ids(&self.s.term));
                continue;
            }
            self.draw();
            if self.s.app.is_media_loading() {
                continue;
            }
            // (no encode worker without a backend: nothing to wait for)
            if self.s.md_enc_rx.is_some() && self.s.drain_md_encode_if_any() {
                // It drew a frame itself: read it.
                self.transmits.extend(transmitted_kitty_ids(&self.s.term));
                continue;
            }
            return;
        }
        panic!("the pipeline did not settle");
    }

    /// The plugin is asked for the canvas again (the declared action `x`), and the answer lands.
    #[track_caller]
    fn redraw(&mut self) {
        self.s.key('x');
        self.transmits.extend(transmitted_kitty_ids(&self.s.term));
        self.pump();
    }

    fn rows(&self) -> Vec<(u16, u16, usize)> {
        kitty_placeholder_rows(&self.s.term)
    }

    fn area(&self) -> (u16, u16, u16, u16) {
        content_area(&self.s)
    }

    /// How many `view` calls the plugin has answered.
    fn views(&self) -> usize {
        self.p.views.load(Ordering::SeqCst)
    }

    /// The cache keys of the canvas pictures the app holds.
    fn canvas_keys(&self) -> Vec<PathBuf> {
        self.s
            .app
            .canvas_image_placements()
            .into_iter()
            .map(|(k, _)| PathBuf::from(k))
            .collect()
    }
}

/// The content hash a canvas cache key ends with.
fn hash_of_key(key: &std::path::Path) -> u64 {
    let s = key.to_string_lossy();
    u64::from_str_radix(s.rsplit('/').next().unwrap(), 16).unwrap()
}

/// The preview frame's content area: (x, y, cols, rows), found from the corner glyphs.
fn content_area(s: &Sim) -> (u16, u16, u16, u16) {
    let buf = s.term.backend().buffer();
    let (w, h) = (buf.area.width, buf.area.height);
    let (mut tl, mut br) = (None, None);
    for y in 0..h {
        for x in 0..w {
            match buf[(x, y)].symbol() {
                "┌" if tl.is_none() => tl = Some((x, y)),
                "┘" => br = Some((x, y)),
                _ => {}
            }
        }
    }
    let (tx, ty) = tl.expect("a frame corner");
    let (bx, by) = br.expect("the opposite corner");
    (tx + 1, ty + 1, bx - tx - 1, by - ty - 1)
}

fn cell(s: &Sim, x: u16, y: u16) -> ratatui::buffer::Cell {
    let (ax, ay, _, _) = content_area(s);
    s.term.backend().buffer()[(ax + x, ay + y)].clone()
}

/// A canvas the size of the request with `under` written at (0, 0): the text a picture covers or
/// leaves alone.
fn base(req: &ViewReq) -> CanvasView {
    let mut c = CanvasView::new(req.cols, req.rows);
    c.set_string(0, 0, "under", PStyle::new());
    c
}

/// A canvas of one 8 x 4 picture at (5, 3), filling its rectangle, in the color `color(n)`.
fn one_sprite(id: &str, color: fn(u32) -> [u8; 3]) -> Sprites {
    Sprites::new(id, move |req, n| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 8, 4);
        c.images.push(placed(1, 5, 3, 8, 4, solid(w, h, color(n))));
        c
    })
}

// ---- where the picture goes ------------------------------------------------------------------

#[test]
fn e2e_canvas_image_is_placed_at_its_cell_rectangle() {
    let o = open_kitty(
        "cvi_rect",
        one_sprite("cvi_rect", |_| [200, 30, 30]),
        90,
        26,
    );
    let (ax, ay, _, _) = o.area();
    let rows = o.rows();
    assert_eq!(
        rows,
        (0..4).map(|i| (ax + 5, ay + 3 + i, 8)).collect::<Vec<_>>(),
        "one placeholder row per picture row, 8 columns wide, at (5, 3) of the content area"
    );
    // Sent once, to the terminal, under one id.
    assert_eq!(o.transmits.len(), 1, "{:?}", o.transmits);
    // The plugin's own cells outside the rectangle are untouched.
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
    assert_eq!(o.canvas_keys().len(), 1);
    assert!(o.s.app.md_image_cache_contains(&o.canvas_keys()[0]));
}

#[test]
fn e2e_canvas_image_key_names_plugin_id_and_content() {
    let o = open_kitty("cvi_key", one_sprite("cvi_key", |_| [1, 2, 3]), 90, 26);
    let key = o.canvas_keys()[0].to_string_lossy().to_string();
    assert!(key.starts_with("canvas://cvi_key/1/"), "{key}");
    assert!(crate::preview::markdown::is_canvas_url(&key));
    assert!(
        crate::preview::markdown::is_synthetic_md_url(&key),
        "never resolved on disk"
    );
}

#[test]
fn e2e_canvas_image_resets_the_text_style_under_it_and_keeps_the_background() {
    let p = Sprites::new("cvi_style", |req, _| {
        let mut c = base(req);
        c.fill(
            CellRect::new(5, 3, 8, 4),
            PStyle::new()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );
        let (w, h) = fill_px(req, 8, 4);
        c.images
            .push(placed(1, 5, 3, 8, 4, solid(w, h, [10, 200, 10])));
        c
    });
    let mut o = open_kitty("cvi_style", p, 90, 26);
    // Before the picture is encoded the plugin's styled cells are there; after, the rectangle's
    // first cells carry the placeholder and no text style.
    o.pump();
    assert_eq!(o.rows().len(), 4);
    let c = cell(&o.s, 5, 3);
    assert!(
        !c.modifier.contains(RModifier::BOLD),
        "attributes reset: {c:?}"
    );
    assert_ne!(c.fg, RColor::Red);
    // A cell of the rectangle the placeholder row does not own keeps the background.
    assert_eq!(cell(&o.s, 12, 6).bg, RColor::Blue, "background kept");
    // Next to the rectangle the plugin's style is unchanged.
    assert_eq!(cell(&o.s, 4, 3).bg, RColor::Reset);
}

#[test]
fn e2e_canvas_two_images_with_different_ids_are_two_pictures() {
    let p = Sprites::new("cvi_two", |req, _| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 6, 3);
        c.images
            .push(placed(1, 2, 2, 6, 3, solid(w, h, [200, 0, 0])));
        c.images
            .push(placed(2, 20, 5, 6, 3, solid(w, h, [0, 0, 200])));
        c
    });
    let o = open_kitty("cvi_two", p, 90, 26);
    let (ax, ay, _, _) = o.area();
    let mut rows = o.rows();
    rows.sort();
    let mut want: Vec<_> = (0..3).map(|i| (ax + 2, ay + 2 + i, 6)).collect();
    want.extend((0..3).map(|i| (ax + 20, ay + 5 + i, 6)));
    want.sort();
    assert_eq!(rows, want);
    let mut ids = o.transmits.clone();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        2,
        "two pictures, two kitty ids: {:?}",
        o.transmits
    );
    let keys = o.canvas_keys();
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1]);
}

#[test]
fn e2e_canvas_same_picture_under_two_ids_is_two_entries_but_same_id_twice_decodes_once() {
    // The same id and content listed twice in one canvas: one key, decoded once.
    let p = Sprites::new("cvi_dup", |req, _| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 4, 2);
        c.images.push(placed(7, 2, 2, 4, 2, solid(w, h, [9, 9, 9])));
        c.images
            .push(placed(7, 20, 2, 4, 2, solid(w, h, [9, 9, 9])));
        c
    });
    let o = open_kitty("cvi_dup", p, 90, 26);
    let keys = o.canvas_keys();
    assert_eq!(keys[0], keys[1]);
    assert_eq!(o.s.app.md_image_cache_len(), 1, "one entry for the one key");
    assert_eq!(o.rows().len(), 4, "both placements are drawn (2 rows each)");
}

// ---- clipping ---------------------------------------------------------------------------------

#[test]
fn e2e_canvas_image_reaching_below_the_area_is_clipped_to_the_visible_band() {
    // A canvas taller than the area; the picture starts 2 rows above the bottom edge and is 6
    // rows tall: only its top 2 rows are visible.
    let p = Sprites::new("cvi_clip_b", |req, _| {
        let mut c = CanvasView::new(req.cols, req.rows + 20);
        let y = req.rows.saturating_sub(2);
        let (w, h) = fill_px(req, 8, 6);
        c.images
            .push(placed(1, 3, y, 8, 6, solid(w, h, [200, 100, 0])));
        c
    });
    let o = open_kitty("cvi_clip_b", p, 90, 26);
    let (ax, ay, _, ah) = o.area();
    let rows = o.rows();
    assert_eq!(
        rows,
        vec![(ax + 3, ay + ah - 2, 8), (ax + 3, ay + ah - 1, 8)],
        "exactly the two visible rows, inside the frame"
    );
    // Nothing leaked into the bottom border or the footer.
    let buf = o.s.term.backend().buffer();
    for x in 0..buf.area.width {
        assert!(
            !buf[(x, ay + ah)].symbol().contains('\u{10eeee}'),
            "border row, col {x}"
        );
    }
}

#[test]
fn e2e_canvas_image_starting_below_the_area_is_skipped() {
    let p = Sprites::new("cvi_out_b", |req, _| {
        let mut c = CanvasView::new(req.cols, req.rows + 10);
        c.set_string(0, 0, "under", PStyle::new());
        let (w, h) = fill_px(req, 4, 2);
        c.images
            .push(placed(1, 1, req.rows + 3, 4, 2, solid(w, h, [1, 1, 1])));
        c
    });
    let o = open_kitty("cvi_out_b", p, 90, 26);
    assert!(
        o.rows().is_empty(),
        "nothing of it is visible: {:?}",
        o.rows()
    );
    assert!(
        o.transmits.is_empty(),
        "and nothing was sent to the terminal"
    );
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
}

#[test]
fn e2e_canvas_image_starting_right_of_the_area_is_skipped() {
    let p = Sprites::new("cvi_out_r", |req, _| {
        let mut c = CanvasView::new(req.cols + 30, req.rows);
        c.set_string(0, 0, "under", PStyle::new());
        let (w, h) = fill_px(req, 4, 2);
        c.images
            .push(placed(1, req.cols + 2, 1, 4, 2, solid(w, h, [1, 1, 1])));
        c
    });
    let o = open_kitty("cvi_out_r", p, 90, 26);
    assert!(o.rows().is_empty());
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
}

#[test]
fn e2e_canvas_image_reaching_past_the_right_edge_is_skipped_whole_and_cells_stay() {
    // The machinery clips bands of rows, not columns: a picture that does not fit across is left
    // out (documented in `ui/canvas.rs`), the plugin's cells under it show.
    let p = Sprites::new("cvi_over_r", |req, _| {
        let mut c = CanvasView::new(req.cols + 30, req.rows);
        c.fill(
            CellRect::new(req.cols.saturating_sub(4), 2, 8, 2),
            PStyle::new().bg(Color::Magenta),
        );
        let (w, h) = fill_px(req, 8, 2);
        c.images.push(placed(
            1,
            req.cols.saturating_sub(4),
            2,
            8,
            2,
            solid(w, h, [1, 1, 1]),
        ));
        c
    });
    let o = open_kitty("cvi_over_r", p, 90, 26);
    assert!(o.rows().is_empty());
    let (_, _, aw, _) = o.area();
    assert_eq!(
        cell(&o.s, aw - 1, 2).bg,
        RColor::Magenta,
        "the plugin's cell shows"
    );
}

#[test]
fn e2e_canvas_image_exactly_filling_the_area_is_drawn_whole() {
    let p = Sprites::new("cvi_full", |req, _| {
        let mut c = CanvasView::new(req.cols, req.rows);
        let (w, h) = fill_px(req, req.cols, req.rows);
        c.images.push(placed(
            1,
            0,
            0,
            req.cols,
            req.rows,
            solid(w, h, [5, 5, 200]),
        ));
        c
    });
    let o = open_kitty("cvi_full", p, 60, 20);
    let (ax, ay, aw, ah) = o.area();
    let rows = o.rows();
    assert_eq!(rows.len(), ah as usize);
    assert!(
        rows.iter().all(|(x, _, c)| *x == ax && *c == aw as usize),
        "{rows:?}"
    );
    assert_eq!(rows[0].1, ay);
}

// ---- the same picture again -------------------------------------------------------------------

#[test]
fn e2e_canvas_same_id_and_content_is_not_decoded_encoded_or_transmitted_again() {
    let mut o = open_kitty(
        "cvi_same",
        one_sprite("cvi_same", |_| [40, 80, 160]),
        90,
        26,
    );
    let key = o.canvas_keys()[0].clone();
    let hash = hash_of_key(&key);
    assert_eq!(decoded_count(hash), 1, "decoded once on the worker");
    let request = o.s.app.md_request_for_test(&key);
    let first_ids = o.transmits.clone();
    assert_eq!(first_ids.len(), 1);
    let views = o.views();
    let kitty_ids_before = o.s.app.md_kitty_ids_for_test(&key);
    for _ in 0..4 {
        o.redraw();
    }
    assert!(
        o.views() >= views + 4,
        "the plugin was asked again each time"
    );
    assert_eq!(o.canvas_keys(), vec![key.clone()], "same key");
    assert_eq!(
        o.s.app.md_request_for_test(&key),
        request,
        "the entry was not replaced"
    );
    assert_eq!(
        decoded_count(hash),
        1,
        "and the worker did not decode it again"
    );
    assert_eq!(o.s.app.md_image_cache_len(), 1);
    assert_eq!(o.transmits, first_ids, "nothing was transmitted again");
    assert_eq!(
        o.s.app.md_kitty_ids_for_test(&key),
        kitty_ids_before,
        "no new kitty id was minted"
    );
    assert_eq!(o.rows().len(), 4, "and it is still drawn");
}

#[test]
fn e2e_canvas_changed_content_under_the_same_id_replaces_the_entry() {
    // Phase 0 is red, phase 1 blue.
    let p = one_sprite(
        "cvi_change",
        |n| if n == 0 { [200, 0, 0] } else { [0, 0, 200] },
    );
    let mut o = open_kitty("cvi_change", p, 90, 26);
    let old = o.canvas_keys()[0].clone();
    assert_eq!(o.transmits.len(), 1);
    o.p.phase.store(1, Ordering::SeqCst);
    o.redraw();
    let new = o.canvas_keys()[0].clone();
    assert_ne!(old, new, "another content, another key");
    assert!(old.to_string_lossy().starts_with("canvas://cvi_change/1/"));
    assert!(new.to_string_lossy().starts_with("canvas://cvi_change/1/"));
    assert!(
        !o.s.app.md_image_cache_contains(&old),
        "the old entry is dropped"
    );
    assert!(o.s.app.md_image_cache_contains(&new));
    assert_eq!(o.s.app.md_image_cache_len(), 1);
    assert_eq!(
        o.transmits.len(),
        2,
        "the new picture was sent: {:?}",
        o.transmits
    );
    assert_ne!(o.transmits[0], o.transmits[1], "under its own id");
    assert_eq!(o.rows().len(), 4);
}

#[test]
fn e2e_canvas_pictures_the_canvas_no_longer_has_are_dropped_and_the_overlay_is_reported_moved() {
    // Phase 0 places the picture, phase 1 none.
    let p = Sprites::new("cvi_gone", |req, n| {
        let mut c = base(req);
        if n == 0 {
            let (w, h) = fill_px(req, 8, 4);
            c.images
                .push(placed(1, 5, 3, 8, 4, solid(w, h, [0, 150, 0])));
        }
        c
    });
    let mut o = open_kitty("cvi_gone", p, 90, 26);
    assert_eq!(o.rows().len(), 4);
    let _ = o.s.app.take_md_overlay_moved();
    // An unchanged redraw does not ask for the clear.
    o.draw();
    assert!(!o.s.app.take_md_overlay_moved(), "nothing moved");
    o.p.phase.store(1, Ordering::SeqCst);
    o.redraw();
    assert!(o.rows().is_empty(), "the picture is gone");
    assert!(
        o.s.app.take_md_overlay_moved(),
        "the vanished picture asks the run loop for its one-time clear"
    );
    assert_eq!(o.s.app.md_image_cache_len(), 0, "and its entry is dropped");
    assert!(o.canvas_keys().is_empty());
}

#[test]
fn e2e_canvas_a_picture_that_moves_is_reported_moved() {
    let p = Sprites::new("cvi_move", |req, n| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 8, 4);
        let x = if n == 0 { 5 } else { 15 };
        c.images
            .push(placed(1, x, 3, 8, 4, solid(w, h, [0, 150, 0])));
        c
    });
    let mut o = open_kitty("cvi_move", p, 90, 26);
    let (ax, _, _, _) = o.area();
    assert_eq!(o.rows()[0].0, ax + 5);
    let key = o.canvas_keys()[0].clone();
    let _ = o.s.app.take_md_overlay_moved();
    o.p.phase.store(1, Ordering::SeqCst);
    o.redraw();
    assert_eq!(o.rows()[0].0, ax + 15, "the same picture, somewhere else");
    assert_eq!(
        o.canvas_keys(),
        vec![key],
        "the same key: the same picture moved"
    );
    assert!(o.s.app.take_md_overlay_moved());
    assert_eq!(
        o.transmits.len(),
        1,
        "moving a picture does not send it again"
    );
}

// ---- failures ---------------------------------------------------------------------------------

#[test]
fn e2e_canvas_undecodable_picture_leaves_the_cells_and_is_not_retried() {
    // Bytes that are an unterminated SVG: the guarded drawing refuses it.
    let junk: Arc<[u8]> = Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' cvi_junk_one"[..]);
    let data = ImageData::Svg {
        data: junk.clone(),
        base_dir: None,
        trusted: false,
    };
    let hash = content_hash(&data);
    let p = Sprites::new("cvi_junk", move |req, _| {
        let mut c = base(req);
        c.fill(CellRect::new(5, 3, 8, 4), PStyle::new().bg(Color::Cyan));
        c.images.push(placed(1, 5, 3, 8, 4, data.clone()));
        c
    });
    let mut o = open_kitty("cvi_junk", p, 90, 26);
    assert!(o.rows().is_empty(), "nothing to draw");
    assert_eq!(cell(&o.s, 5, 3).bg, RColor::Cyan, "the plugin's cells stay");
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
    assert_eq!(decoded_count(hash), 1, "tried once");
    for _ in 0..3 {
        o.redraw();
    }
    assert_eq!(
        decoded_count(hash),
        1,
        "the failure is remembered: not tried again on later redraws"
    );
    assert!(o.rows().is_empty());
    assert_eq!(cell(&o.s, 5, 3).bg, RColor::Cyan);
    // The failed entry is held, so the next job does not decode it.
    assert!(o.s.app.md_image_cache_contains(&o.canvas_keys()[0]));
}

#[test]
fn e2e_canvas_garbage_encoded_bytes_fail_quietly() {
    let p = Sprites::new("cvi_garbage", |req, _| {
        let mut c = base(req);
        c.images.push(placed(
            1,
            5,
            3,
            8,
            4,
            ImageData::Encoded(Arc::from(&b"definitely not an image"[..])),
        ));
        c
    });
    let o = open_kitty("cvi_garbage", p, 90, 26);
    assert!(o.rows().is_empty());
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
    assert!(o.s.app.is_canvas_preview(), "the canvas itself is shown");
}

#[test]
fn e2e_canvas_missing_file_picture_fails_quietly() {
    let p = Sprites::new("cvi_nofile", |req, _| {
        let mut c = base(req);
        c.images.push(placed(
            1,
            5,
            3,
            8,
            4,
            ImageData::File("/definitely/not/here.png".into()),
        ));
        c
    });
    let o = open_kitty("cvi_nofile", p, 90, 26);
    assert!(o.rows().is_empty());
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
}

#[test]
fn e2e_canvas_oversized_picture_is_refused_and_the_rest_still_draws() {
    // 20000 x 20000 pixels announced by the header of a PNG is over the decoder's limit: the
    // picture fails, the second one is unaffected.
    let big = {
        use image::ImageEncoder;
        let mut out = Vec::new();
        // A real (tiny) PNG whose IHDR is then patched to a huge size.
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&[0, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        out[16..20].copy_from_slice(&20000u32.to_be_bytes());
        out[20..24].copy_from_slice(&20000u32.to_be_bytes());
        Arc::<[u8]>::from(out)
    };
    let p = Sprites::new("cvi_big", move |req, _| {
        let mut c = base(req);
        c.images
            .push(placed(1, 1, 1, 4, 2, ImageData::Encoded(big.clone())));
        let (w, h) = fill_px(req, 4, 2);
        c.images
            .push(placed(2, 20, 1, 4, 2, solid(w, h, [3, 3, 3])));
        c
    });
    let o = open_kitty("cvi_big", p, 90, 26);
    let (ax, ay, _, _) = o.area();
    assert_eq!(o.rows(), vec![(ax + 20, ay + 1, 4), (ax + 20, ay + 2, 4)]);
}

// ---- SVG --------------------------------------------------------------------------------------

#[test]
fn e2e_canvas_svg_from_a_plugin_is_drawn_by_the_supervised_child_path() {
    let svg: Arc<[u8]> = Arc::from(
        &br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80"><rect width="80" height="80" fill="#3355aa"/><!-- cvi_svg_child --></svg>"##[..],
    );
    let bytes = svg.clone();
    // The plugin claims `trusted: true`: for a plugin of this kind the claim is not believed.
    let p = Sprites::new("cvi_svg", move |req, _| {
        let mut c = base(req);
        c.images.push(placed(
            1,
            5,
            3,
            8,
            4,
            ImageData::Svg {
                data: bytes.clone(),
                base_dir: None,
                trusted: true,
            },
        ));
        c
    });
    let mut o = open_kitty("cvi_svg", p, 90, 26);
    assert!(
        crate::preview::svg::seen_for_tests::untrusted_count(&svg) >= 1,
        "drawn by the supervised path"
    );
    assert!(!o.rows().is_empty(), "and shown: {:?}", o.rows());
    let n = crate::preview::svg::seen_for_tests::untrusted_count(&svg);
    o.redraw();
    o.redraw();
    assert_eq!(
        crate::preview::svg::seen_for_tests::untrusted_count(&svg),
        n,
        "the same SVG sent again is not drawn again"
    );
}

// ---- no pictures ------------------------------------------------------------------------------

#[test]
fn e2e_canvas_without_an_image_backend_keeps_the_cells_and_decodes_nothing() {
    let p = Sprites::new("cvi_nopicker", |req, _| {
        let mut c = base(req);
        c.fill(CellRect::new(5, 3, 8, 4), PStyle::new().bg(Color::Cyan));
        let (w, h) = fill_px(req, 8, 4);
        c.images.push(placed(1, 5, 3, 8, 4, solid(w, h, [1, 2, 3])));
        c
    });
    let o = open_on("cvi_nopicker", p, 90, 26, |s| s);
    assert!(o.s.app.is_canvas_preview());
    assert_eq!(cell(&o.s, 5, 3).bg, RColor::Cyan, "the plugin's cells stay");
    assert_eq!(o.s.app.md_image_cache_len(), 0, "nothing was decoded");
    assert!(o.canvas_keys().is_empty());
}

#[test]
fn e2e_canvas_on_a_halfblocks_backend_draws_the_picture_as_halfblocks() {
    let p = Sprites::new("cvi_half", |req, _| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 8, 4);
        c.images
            .push(placed(1, 5, 3, 8, 4, solid(w, h, [250, 10, 10])));
        c
    });
    let o = open_on("cvi_half", p, 90, 26, |s| s.with_media());
    assert!(o.rows().is_empty(), "no kitty placeholders");
    // A solid picture is drawn as halfblock cells whose colors are the picture's (a half block of
    // one color shows as a space on that background).
    let c = cell(&o.s, 8, 5);
    assert_eq!(c.bg, RColor::Rgb(250, 10, 10), "{c:?}");
    assert_eq!(c.fg, RColor::Rgb(250, 10, 10), "{c:?}");
    assert_ne!(
        cell(&o.s, 4, 5).bg,
        RColor::Rgb(250, 10, 10),
        "outside the rectangle"
    );
    assert_eq!(cell(&o.s, 0, 0).symbol(), "u");
}

// ---- animation --------------------------------------------------------------------------------

#[test]
fn e2e_canvas_frames_animate_under_one_kitty_id() {
    let p = Sprites::new("cvi_frames", |req, _| {
        let mut c = base(req);
        let (w, h) = fill_px(req, 8, 4);
        c.images.push(placed(
            1,
            5,
            3,
            8,
            4,
            ImageData::Frames(vec![
                (solid(w, h, [200, 0, 0]), Duration::ZERO),
                (solid(w, h, [0, 200, 0]), Duration::ZERO),
                (solid(w, h, [0, 0, 200]), Duration::ZERO),
            ]),
        ));
        c
    });
    let mut o = open_kitty("cvi_frames", p, 90, 26);
    assert_eq!(o.transmits.len(), 1);
    // The first call only starts the timer; zero delays advance on each call after.
    let _ = o.s.app.advance_md_gifs_if_due();
    for _ in 0..4 {
        assert!(o.s.app.advance_md_gifs_if_due(), "the frame advances");
        o.pump();
    }
    assert!(
        o.transmits.len() >= 4,
        "every frame is sent: {:?}",
        o.transmits
    );
    let mut ids = o.transmits.clone();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 1, "always under the same id: {:?}", o.transmits);
    assert_eq!(o.rows().len(), 4);
}

#[test]
fn e2e_canvas_empty_frames_list_fails_quietly() {
    let p = Sprites::new("cvi_noframes", |req, _| {
        let mut c = base(req);
        c.images
            .push(placed(1, 5, 3, 8, 4, ImageData::Frames(Vec::new())));
        c
    });
    let o = open_kitty("cvi_noframes", p, 90, 26);
    assert!(o.rows().is_empty());
    assert!(o.s.app.is_canvas_preview());
}

// ---- lifetime ---------------------------------------------------------------------------------

#[test]
fn e2e_canvas_pictures_go_with_the_show() {
    let mut o = open_kitty("cvi_life", one_sprite("cvi_life", |_| [7, 7, 7]), 90, 26);
    assert_eq!(o.s.app.md_image_cache_len(), 1);
    // Leave to the tree, and open a file nothing draws as a canvas.
    std::fs::write(o._dir.join("b.txt"), b"hello").unwrap();
    o.s.esc();
    o.s.select("b.txt");
    o.s.enter();
    o.pump();
    assert_eq!(
        o.s.app.md_image_cache_len(),
        0,
        "no entry outlives the show"
    );
    assert!(o.canvas_keys().is_empty());
}

#[test]
fn e2e_canvas_resize_asks_again_and_keeps_the_picture_without_resending_it() {
    let mut o = open_kitty(
        "cvi_resize",
        one_sprite("cvi_resize", |_| [70, 70, 70]),
        90,
        26,
    );
    let key = o.canvas_keys()[0].clone();
    let sent = o.transmits.clone();
    o.s.resize(100, 30);
    o.pump();
    assert_eq!(o.canvas_keys(), vec![key], "same content: same key");
    assert_eq!(o.transmits, sent, "and not sent again");
    assert_eq!(o.rows().len(), 4);
}
