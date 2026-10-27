//! What the host keeps of a canvas answer is bounded by the request, not by what the plugin sent
//! (`plugin_host::well_formed`, `CanvasView::clipped`): the cells beyond the size asked for (and
//! at least a floor, for the first answer to the 80 x 1 placeholder request) are dropped before
//! the answer lands, so the memory and the `check()` work follow the viewport. A resize changes
//! the request, and the host asks the plugin again for the new size.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_plugin::prelude::*;
use konoma_plugin::view::{CanvasImage, CanvasIssue, CanvasView, CellRect, ImageData};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::plugin_host::{
    canvas_keep_size, well_formed, CANVAS_KEEP_MIN_COLS, CANVAS_KEEP_MIN_ROWS,
};
use super::*;
use crate::config::Config;
use crate::plugins::test_plugins;
use crate::test_support::unique_tmp;

/// The size the stand-in plugin always answers with, whatever it is asked for.
const SENT: (u16, u16) = (2000, 1000);

struct Big {
    id: String,
    reqs: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl Plugin for Big {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.big"]))
            .shows(ViewKind::Canvas)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(BigSession {
            reqs: self.reqs.clone(),
        }))
    }
}

struct BigSession {
    reqs: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl Session for BigSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.reqs.lock().unwrap().push((req.cols, req.rows));
        let mut c = CanvasView::new(SENT.0, SENT.1);
        c.set_string(0, 0, "A", konoma_plugin::Style::new());
        c.set_string(SENT.0 - 1, SENT.1 - 1, "Z", konoma_plugin::Style::new());
        Ok(View::Canvas(c))
    }
}

struct H {
    app: App,
    term: Terminal<TestBackend>,
    reqs: Arc<Mutex<Vec<(u16, u16)>>>,
    _dir: crate::test_support::TmpDir,
}

impl H {
    fn open(name: &str, cols: u16, rows: u16) -> H {
        let dir = unique_tmp(&format!("konoma_canvas_bounds_{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.big"), b"x").unwrap();
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let marker = test_plugins::register(
            name,
            Arc::new(Big {
                id: name.to_string(),
                reqs: reqs.clone(),
            }),
        );
        let cfg: Config = test_plugins::marker_config(&marker);
        let app = App::new(dir.canonicalize().unwrap(), cfg).expect("App::new");
        let term = Terminal::new(TestBackend::new(cols, rows)).expect("terminal");
        let mut h = H {
            app,
            term,
            reqs,
            _dir: dir,
        };
        h.draw();
        let i = h
            .app
            .tab
            .entries
            .iter()
            .position(|e| e.path.to_string_lossy().ends_with("a.big"))
            .expect("a.big is listed");
        h.app.tab.selected = i;
        crate::handle_key(
            &mut h.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .expect("key");
        // The first frame measures the area and asks the canvas again; draw until it settles.
        for _ in 0..4 {
            h.draw();
        }
        h
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| crate::ui::render(f, app)).expect("draw");
    }

    fn held(&self) -> (u16, u16, usize) {
        let c = self.app.canvas_view().expect("a canvas on screen");
        (c.cols, c.rows, c.cells.len())
    }
}

#[test]
fn the_floor_and_the_keep_size_are_pinned() {
    assert_eq!((CANVAS_KEEP_MIN_COLS, CANVAS_KEEP_MIN_ROWS), (320, 128));
    assert_eq!(canvas_keep_size(&ViewReq::new(80, 1)), (320, 128));
    assert_eq!(canvas_keep_size(&ViewReq::new(0, 0)), (320, 128));
    assert_eq!(canvas_keep_size(&ViewReq::new(400, 150)), (400, 150));
    assert_eq!(canvas_keep_size(&ViewReq::new(400, 20)), (400, 128));
    assert_eq!(
        canvas_keep_size(&ViewReq::new(u16::MAX, u16::MAX)),
        (u16::MAX, u16::MAX)
    );
}

#[test]
fn a_canvas_much_bigger_than_the_request_is_cut_before_it_lands() {
    let mut h = H::open("cb_cut", 90, 26);
    // The plugin sent 2000 x 1000 (2M cells); the host holds the floor, not the 2M.
    let (cols, rows, cells) = h.held();
    assert_eq!(
        (cols, rows),
        (320, 128),
        "cut to the floor, far under what was sent"
    );
    assert_eq!(cells, 320 * 128);
    assert!(cells < SENT.0 as usize * SENT.1 as usize / 40);
    // What the screen shows is the same: the top-left corner of the canvas.
    let buf = h.term.backend().buffer().clone();
    let all: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(all.contains('A'), "the corner cell is drawn");
    assert!(!all.contains('Z'), "the far corner was never visible");
    h.draw();
}

#[test]
fn a_resize_asks_again_and_the_new_area_is_covered_by_the_new_answer() {
    let mut h = H::open("cb_resize", 90, 26);
    assert_eq!(h.reqs.lock().unwrap().last().copied(), Some((88, 22)));
    assert_eq!(h.held().0, 320);
    // A terminal bigger than the floor: the request grows past it, so the cut follows the request.
    h.term.backend_mut().resize(420, 160);
    for _ in 0..4 {
        h.draw();
    }
    let area = (h.app.tab.preview_cols, h.app.tab.preview_viewport);
    assert!(
        area.0 > 320 && area.1 > 128,
        "the area is past the floor: {area:?}"
    );
    assert_eq!(
        h.reqs.lock().unwrap().last().copied(),
        Some(area),
        "the host asked the plugin again for the new area"
    );
    let (cols, rows, cells) = h.held();
    assert_eq!((cols, rows), area, "the answer covers the whole new area");
    assert_eq!(cells, area.0 as usize * area.1 as usize);
}

#[test]
fn a_canvas_that_fits_is_kept_whole() {
    let req = ViewReq::new(80, 24);
    let mut c = CanvasView::new(80, 24);
    c.set_string(0, 0, "hi", konoma_plugin::Style::new());
    let before = c.cells.clone();
    let View::Canvas(got) =
        well_formed("p", &req, View::Canvas(c)).unwrap_or_else(|_| panic!("ok"))
    else {
        panic!("a canvas")
    };
    assert_eq!((got.cols, got.rows), (80, 24));
    assert_eq!(got.cells, before);
}

#[test]
fn a_bad_cell_inside_what_is_kept_is_refused_and_one_cut_away_is_not_judged() {
    let req = ViewReq::new(80, 24);
    let mut c = CanvasView::new(1000, 1000);
    c.cell_mut(900, 900).unwrap().symbol = "\u{1b}".to_string();
    assert!(well_formed("p", &req, View::Canvas(c.clone())).is_ok());
    c.cell_mut(5, 5).unwrap().symbol = "\u{1b}".to_string();
    assert!(well_formed("p", &req, View::Canvas(c)).is_err());
}

#[test]
fn the_size_rules_are_judged_on_what_was_sent() {
    let req = ViewReq::new(80, 24);
    let mut c = CanvasView::new(1000, 1000);
    c.cells.pop();
    assert!(
        well_formed("p", &req, View::Canvas(c)).is_err(),
        "cell count"
    );
    let mut big = CanvasView::new(0, 0);
    big.cols = 4097;
    big.rows = 1;
    assert!(well_formed("p", &req, View::Canvas(big)).is_err(), "area");
}

#[test]
fn placements_wholly_past_the_kept_part_are_dropped_and_the_canvas_is_not_refused() {
    let req = ViewReq::new(80, 24);
    let mut c = CanvasView::new(1000, 1000);
    let px = ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: Arc::new(vec![0; 4]),
    };
    c.images = vec![
        CanvasImage::new(1, CellRect::new(2, 2, 4, 4), px.clone()),
        CanvasImage::new(2, CellRect::new(900, 900, 4, 4), px),
    ];
    let View::Canvas(got) =
        well_formed("p", &req, View::Canvas(c)).unwrap_or_else(|_| panic!("ok"))
    else {
        panic!("a canvas")
    };
    assert_eq!(got.images.iter().map(|i| i.id).collect::<Vec<_>>(), vec![1]);
    let _: Option<CanvasIssue> = got.check().err();
}

/// Answers a big canvas with a picture that starts on the screen and runs past the bottom edge of
/// what was asked for, and one that runs past the right edge.
struct Pics;

impl Plugin for Pics {
    fn manifest(&self) -> Manifest {
        Manifest::new("pics", "0.1.0")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Canvas)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(PicsSession))
    }
}

struct PicsSession;

impl Session for PicsSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let mut c = CanvasView::new(req.cols.saturating_add(40), req.rows.saturating_add(40));
        let px = |v: u8| ImageData::Rgba {
            width: 2,
            height: 2,
            pixels: Arc::new(vec![v; 16]),
        };
        // Starts two rows above the bottom of the request, 6 rows tall.
        c.images.push(CanvasImage::new(
            1,
            CellRect::new(2, req.rows.saturating_sub(2), 4, 6),
            px(200),
        ));
        // Starts two columns before the right edge of the request, 4 wide.
        c.images.push(CanvasImage::new(
            2,
            CellRect::new(req.cols.saturating_sub(2), 1, 4, 2),
            px(90),
        ));
        Ok(View::Canvas(c))
    }
}

#[test]
fn a_picture_crossing_the_bottom_edge_of_a_request_above_the_floor_is_still_drawn_as_a_band() {
    // A terminal bigger than the floor (320 x 128), so the kept part IS the request.
    let dir = unique_tmp("konoma_canvas_bounds_pics");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pic"), b"x").unwrap();
    let marker = test_plugins::register("pics", Arc::new(Pics));
    let cfg: Config = test_plugins::marker_config(&marker);
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).expect("App::new");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    // Pictures are decoded and placed only with an image backend and the encode worker (the
    // encodes themselves are never run here: the placements are what the renderer is handed).
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (etx, _erx) = std::sync::mpsc::channel();
    app.attach_md_encoder(etx);
    let mut term = Terminal::new(TestBackend::new(340, 150)).expect("terminal");
    fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
        term.draw(|f| crate::ui::render(f, app)).expect("draw");
    }
    draw(&mut term, &mut app);
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.to_string_lossy().ends_with("a.pic"))
        .expect("a.pic is listed");
    app.tab.selected = i;
    crate::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).expect("key");
    // The answers of the workers (the plugin's, then the pictures') are applied as the run loop
    // does; draw in between so the first measure asks for the right size.
    for _ in 0..30 {
        draw(&mut term, &mut app);
        if !app.is_media_loading() {
            if app.canvas_place_count() == 2 {
                break;
            }
            continue;
        }
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("a worker answers");
        app.apply_media(res);
    }
    let (cols, rows) = (app.tab.preview_cols, app.tab.preview_viewport as u16);
    let (keep_cols, keep_rows) = canvas_keep_size(&ViewReq::new(cols, rows));
    assert!(
        rows > CANVAS_KEEP_MIN_ROWS && cols > CANVAS_KEEP_MIN_COLS,
        "the request is above the floor ({cols} x {rows})"
    );
    assert_eq!((keep_cols, keep_rows), (cols, rows));
    let c = app.canvas_view().expect("a canvas");
    assert_eq!((c.cols, c.rows), (cols, rows), "cut to the request");
    assert_eq!(c.images.len(), 2, "both placements start inside: both kept");
    let inner = ratatui::layout::Rect::new(0, 0, cols, rows);
    let n = app.canvas_place_count();
    assert_eq!(n, 2);
    let mut seen = Vec::new();
    for k in 0..n {
        let p = app.canvas_place(k).expect("place");
        seen.push((p.id, crate::ui::canvas::visible_rect(inner, p.rect)));
    }
    seen.sort_by_key(|(id, _)| *id);
    // The bottom one reaches the renderer as a band of its 2 visible rows, exactly as on the uncut
    // canvas; the one past the right edge is not drawn there either.
    assert_eq!(
        seen,
        vec![
            (1, Some(ratatui::layout::Rect::new(2, rows - 2, 4, 2))),
            (2, None)
        ]
    );
}
