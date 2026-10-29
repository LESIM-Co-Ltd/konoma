//! The pictures a canvas places, in the standalone app: drawn over the canvas cells through the
//! shared machinery, on a `TestBackend` with an injected picker (never a real terminal).

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::app::{
    delete_placed_pictures, open_file, run_with, run_with_pictures, AppCx, AppEvent, AppKeys,
    EventSource, InProcessSvg, Next, Pictures,
};
use konoma_plugin::canvas_pictures::SvgDraw;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{CanvasImage, ImageData};
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::Color as RColor;
use ratatui::Terminal;
use ratatui_image::picker::{Picker, ProtocolType};

const HALF: &str = "\u{2580}";
const PLACEHOLDER: &str = "\u{10EEEE}";
/// The longest any test waits for a state that must come (a failure bound, never an expectation).
const DEADLINE: Duration = Duration::from_secs(20);
/// `LIVE_CANVAS_KITTY` is process-wide: the tests that read it take turns.
static KITTY: Mutex<()> = Mutex::new(());

// ---- the plugin: a canvas made by a script of the call number ----

type Script = Arc<dyn Fn(usize) -> Vec<CanvasImage> + Send + Sync>;

struct PicPlugin {
    script: Script,
    calls: Arc<AtomicUsize>,
}

impl Plugin for PicPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("pic-test", "0.0.1")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Canvas)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(PicSession {
            script: Arc::clone(&self.script),
            calls: Arc::clone(&self.calls),
        }))
    }
}

struct PicSession {
    script: Script,
    calls: Arc<AtomicUsize>,
}

impl Session for PicSession {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut c = CanvasView::new(req.cols, req.rows);
        for y in 0..req.rows {
            c.set_string(0, y, &"x".repeat(usize::from(req.cols)), Style::new());
        }
        c.images = (self.script)(n);
        Ok(View::Canvas(c))
    }
}

// ---- the backend: a TestBackend whose buffer the event source can look at ----

#[derive(Default)]
struct Shared {
    /// The symbol of the probe cell after every flushed frame.
    history: Vec<String>,
    clears: usize,
}

struct Probe {
    inner: Arc<Mutex<TestBackend>>,
    shared: Arc<Mutex<Shared>>,
    cell: (u16, u16),
}

impl Backend for Probe {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.lock().unwrap().draw(content)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(Position::new(0, 0))
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _p: P) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.lock().unwrap().clear()
    }
    fn clear_region(&mut self, t: ClearType) -> Result<(), Self::Error> {
        if matches!(t, ClearType::All) {
            self.shared.lock().unwrap().clears += 1;
        }
        self.inner.lock().unwrap().clear_region(t)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        let sym = self.inner.lock().unwrap().buffer()[self.cell]
            .symbol()
            .to_string();
        self.shared.lock().unwrap().history.push(sym);
        Ok(())
    }
}

// ---- the events: wait for a state of the screen, then do the next thing ----

struct Step {
    until: Box<dyn Fn(&Buffer) -> bool>,
    /// Run when the state came, before `then`.
    act: Option<Box<dyn FnOnce()>>,
    /// `None` ends the run.
    then: Option<AppEvent>,
}

fn step(until: impl Fn(&Buffer) -> bool + 'static, then: Option<AppEvent>) -> Step {
    Step {
        until: Box::new(until),
        act: None,
        then,
    }
}

/// Every `wait` the loop asked the event source for, in order (`None`: no wake needed).
type Waits = Arc<Mutex<Vec<Option<Duration>>>>;

struct Driver {
    screen: Arc<Mutex<TestBackend>>,
    steps: VecDeque<Step>,
    started: Instant,
    waits: Waits,
}

impl EventSource for Driver {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        self.waits.lock().unwrap().push(wait);
        let Some(front) = self.steps.front() else {
            return Next::End;
        };
        let ready = (front.until)(self.screen.lock().unwrap().buffer());
        if ready {
            let s = self.steps.pop_front().unwrap();
            if let Some(a) = s.act {
                a();
            }
            return match s.then {
                Some(e) => Next::Event(e),
                None => Next::End,
            };
        }
        if self.started.elapsed() >= DEADLINE {
            let b = self.screen.lock().unwrap();
            let rows: Vec<String> = (0..b.buffer().area.height)
                .map(|y| (0..14).map(|x| sym(b.buffer(), x, y)).collect())
                .collect();
            panic!("the screen never got there: {rows:#?}");
        }
        // Give the loop its turn (it draws between two calls), without spinning.
        std::thread::sleep(
            wait.unwrap_or(Duration::from_millis(3))
                .min(Duration::from_millis(3)),
        );
        Next::Timeout
    }
}

// ---- helpers ----

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn cx() -> AppCx {
    AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
        ..AppCx::default()
    }
}

fn kitty_picker() -> Picker {
    let mut p = Picker::halfblocks();
    p.set_protocol_type(ProtocolType::Kitty);
    p
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> ImageData {
    let px: Vec<u8> = (0..w * h)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    ImageData::Rgba {
        width: w,
        height: h,
        pixels: Arc::new(px),
    }
}

const RECT: (u16, u16, u16, u16) = (2, 1, 4, 2);

fn pic(id: u64, data: ImageData) -> CanvasImage {
    CanvasImage::new(id, CellRect::new(RECT.0, RECT.1, RECT.2, RECT.3), data)
}

fn unchanged(id: u64) -> CanvasImage {
    CanvasImage::unchanged(id, CellRect::new(RECT.0, RECT.1, RECT.2, RECT.3))
}

fn sym(b: &Buffer, x: u16, y: u16) -> String {
    b[(x, y)].symbol().to_string()
}

fn drawn(b: &Buffer) -> bool {
    sym(b, RECT.0, RECT.1) != "x"
}

/// The color of the cell of a solid halfblock picture (it is a space on a background when both
/// halves are the same).
fn pixel(c: &Cell) -> RColor {
    if c.symbol() == HALF {
        c.fg
    } else {
        c.bg
    }
}

fn placeholder(b: &Buffer) -> bool {
    sym(b, RECT.0, RECT.1).contains(PLACEHOLDER)
}

/// A file for the plugin to open, in a directory that lives as long as the guard.
fn pic_file() -> (konoma_test_support::TmpDir, PathBuf) {
    let dir = unique_tmp("app_pictures");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.pic");
    std::fs::write(&p, "x").unwrap();
    (dir, p)
}

/// What a run left behind.
struct Out {
    shared: Arc<Mutex<Shared>>,
    screen: Arc<Mutex<TestBackend>>,
    calls: Arc<AtomicUsize>,
    waits: Waits,
}

impl Out {
    fn history(&self) -> Vec<String> {
        self.shared.lock().unwrap().history.clone()
    }
    fn clears(&self) -> usize {
        self.shared.lock().unwrap().clears
    }
    fn cell(&self, x: u16, y: u16) -> Cell {
        self.screen.lock().unwrap().buffer()[(x, y)].clone()
    }
}

/// Run the app over a canvas `script` makes, on a 30x8 screen, with `pictures` (made from the
/// file's path) and the `steps`. The probe cell is the first cell of the picture.
fn run_pic(
    script: impl Fn(usize) -> Vec<CanvasImage> + Send + Sync + 'static,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: impl FnOnce(&Arc<AtomicUsize>) -> Vec<Step>,
) -> Out {
    run_pic_waits(script, pictures, |c, _| steps(c))
}

/// [`run_pic`] whose steps can also read the waits the loop has asked for so far.
fn run_pic_waits(
    script: impl Fn(usize) -> Vec<CanvasImage> + Send + Sync + 'static,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: impl FnOnce(&Arc<AtomicUsize>, &Waits) -> Vec<Step>,
) -> Out {
    let waits = Waits::default();
    let (_dir, path) = pic_file();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = PicPlugin {
        script: Arc::new(script),
        calls: Arc::clone(&calls),
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(30, 8)));
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut term = Terminal::new(Probe {
        inner: Arc::clone(&screen),
        shared: Arc::clone(&shared),
        cell: (RECT.0, RECT.1),
    })
    .unwrap();
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: steps(&calls, &waits).into(),
        started: Instant::now(),
        waits: Arc::clone(&waits),
    };
    let pictures = pictures(&opened.path);
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx(),
        &AppKeys::defaults(),
        pictures,
    )
    .expect("runs");
    Out {
        shared,
        screen,
        calls,
        waits,
    }
}

fn half(path: &Path) -> Option<Pictures> {
    Some(Pictures::new(Picker::halfblocks(), "pic-test", path, 800))
}

/// A buffer the kitty deletes are written to.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

// ---- tests ----

#[test]
fn a_static_picture_is_drawn_over_the_canvas_cells_and_only_there() {
    let out = run_pic(
        |_| vec![pic(1, solid(64, 64, [255, 0, 0]))],
        half,
        |_| vec![step(drawn, None)],
    );
    for y in 1..3 {
        for x in 2..6 {
            assert_ne!(out.cell(x, y).symbol(), "x", "({x},{y})");
        }
    }
    // The cells around it are the plugin's.
    assert_eq!(out.cell(1, 1).symbol(), "x");
    assert_eq!(out.cell(6, 1).symbol(), "x");
    assert_eq!(out.cell(2, 0).symbol(), "x");
    assert_eq!(out.cell(2, 3).symbol(), "x");
    assert_eq!(pixel(&out.cell(2, 1)), RColor::Rgb(255, 0, 0));
}

#[test]
fn without_pictures_the_canvas_cells_stay_whatever_the_canvas_places() {
    let (_dir, path) = pic_file();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = PicPlugin {
        script: Arc::new(|_| vec![pic(1, solid(64, 64, [255, 0, 0]))]),
        calls: Arc::clone(&calls),
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
    let mut it = vec![AppEvent::Resize, AppEvent::Resize].into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx(),
        &AppKeys::defaults(),
    )
    .unwrap();
    let b = term.backend().buffer();
    assert_eq!(sym(b, 2, 1), "x");
    assert!(!drawn(b));
    // The same through the loop that takes pictures, given none.
    let out = run_pic(
        |_| vec![pic(1, solid(64, 64, [255, 0, 0]))],
        |_| None,
        |calls| {
            let c = Arc::clone(calls);
            vec![
                step(|_| true, Some(AppEvent::Resize)),
                step(move |_| c.load(Ordering::SeqCst) >= 2, None),
            ]
        },
    );
    assert_eq!(out.cell(2, 1).symbol(), "x");
}

#[test]
fn a_gif_advances_frame_by_frame_with_its_own_delays() {
    let frames = vec![
        (solid(64, 64, [255, 0, 0]), Duration::from_millis(120)),
        (solid(64, 64, [0, 0, 255]), Duration::from_millis(120)),
    ];
    let color = |b: &Buffer, c: RColor| drawn(b) && pixel(&b[(RECT.0, RECT.1)]) == c;
    run_pic(
        move |_| vec![pic(1, ImageData::Frames(frames.clone()))],
        half,
        |_| {
            vec![
                step(
                    move |b| color(b, RColor::Rgb(255, 0, 0)),
                    Some(AppEvent::Resize),
                ),
                step(
                    move |b| color(b, RColor::Rgb(0, 0, 255)),
                    Some(AppEvent::Resize),
                ),
                // and round again to the first
                step(move |b| color(b, RColor::Rgb(255, 0, 0)), None),
            ]
        },
    );
}

/// Draws SVGs like the app does and says what size it was asked for.
struct Recording {
    max_px: Arc<AtomicU32>,
}

impl SvgDraw for Recording {
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.max_px.store(max_px, Ordering::SeqCst);
        InProcessSvg.untrusted(data, base, roots, max_px, c)
    }
    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.max_px.store(max_px, Ordering::SeqCst);
        InProcessSvg.embedded(data, max_px, c)
    }
}

const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#ff0000"/></svg>"##;

fn svg_data(trusted: bool) -> ImageData {
    ImageData::Svg {
        data: Arc::from(RED_SVG.as_bytes()),
        base_dir: None,
        trusted,
    }
}

#[test]
fn an_svg_is_drawn_in_process_at_the_configured_size() {
    for trusted in [true, false] {
        let max_px = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&max_px);
        let out = run_pic(
            move |_| vec![pic(1, svg_data(trusted))],
            move |p| {
                Some(
                    Pictures::new(Picker::halfblocks(), "pic-test", p, 321)
                        .with_svg(Arc::new(Recording { max_px: seen })),
                )
            },
            |_| vec![step(drawn, None)],
        );
        let c = out.cell(RECT.0, RECT.1);
        assert_ne!(c.symbol(), "x", "trusted={trusted}");
        let RColor::Rgb(r, g, b) = pixel(&c) else {
            panic!("not rgb: {:?}", c)
        };
        assert!(r > 200 && g < 60 && b < 60, "{r} {g} {b} trusted={trusted}");
        // A plugin's own (trusted) SVG is drawn by konoma's rasterizer without the guarded
        // drawer; any other goes through the guarded in-process drawer at the configured size.
        let want = if trusted { 0 } else { 321 };
        assert_eq!(max_px.load(Ordering::SeqCst), want, "trusted={trusted}");
    }
}

#[test]
fn an_unchanged_picture_keeps_its_place_and_is_not_decoded_again() {
    let out = run_pic(
        |n| {
            if n == 0 {
                vec![pic(1, solid(64, 64, [255, 0, 0]))]
            } else {
                vec![unchanged(1)]
            }
        },
        half,
        |calls| {
            let c = Arc::clone(calls);
            vec![
                step(drawn, Some(AppEvent::Resize)),
                // the session answered again, and the picture is still there
                step(move |b| c.load(Ordering::SeqCst) >= 3 && drawn(b), None),
            ]
        },
    );
    assert!(out.calls.load(Ordering::SeqCst) >= 3);
    // From the first frame it was drawn on, it was never gone.
    let h = out.history();
    let first = h.iter().position(|s| s != "x").expect("drawn");
    assert!(h[first..].iter().all(|s| s != "x"), "{h:?}");
}

#[test]
fn an_unchanged_picture_that_moved_is_drawn_at_its_new_place_from_the_data_held() {
    let out = run_pic(
        |n| {
            if n == 0 {
                vec![pic(1, solid(64, 64, [255, 0, 0]))]
            } else {
                vec![CanvasImage::unchanged(1, CellRect::new(10, 3, 4, 2))]
            }
        },
        half,
        |_| {
            vec![
                step(drawn, Some(AppEvent::Resize)),
                step(|b| sym(b, 10, 3) != "x", None),
            ]
        },
    );
    assert_ne!(out.cell(10, 3).symbol(), "x");
    assert_eq!(pixel(&out.cell(10, 3)), RColor::Rgb(255, 0, 0));
    // Not at the old place any more.
    assert_eq!(out.cell(RECT.0, RECT.1).symbol(), "x");
}

#[test]
fn changed_content_does_not_blink() {
    let out = run_pic(
        |n| {
            if n == 0 {
                vec![pic(1, solid(64, 64, [255, 0, 0]))]
            } else {
                // a new picture under the same id, each call
                vec![pic(1, solid(64, 64, [0, 0, 255]))]
            }
        },
        half,
        |_| {
            let blue =
                |b: &Buffer| drawn(b) && pixel(&b[(RECT.0, RECT.1)]) == RColor::Rgb(0, 0, 255);
            vec![step(drawn, Some(AppEvent::Resize)), step(blue, None)]
        },
    );
    let h = out.history();
    let first = h.iter().position(|s| s != "x").expect("drawn");
    assert!(h[first..].iter().all(|s| s != "x"), "blinked: {h:?}");
    assert_eq!(pixel(&out.cell(RECT.0, RECT.1)), RColor::Rgb(0, 0, 255));
}

#[test]
fn a_picture_that_goes_clears_the_screen_and_deletes_its_kitty_image() {
    let _turn = KITTY.lock().unwrap_or_else(|e| e.into_inner());
    let sink = Sink::default();
    let into = sink.clone();
    let out = run_pic(
        |n| {
            if n == 0 {
                vec![pic(1, solid(64, 64, [255, 0, 0]))]
            } else {
                Vec::new()
            }
        },
        move |p| Some(Pictures::new(kitty_picker(), "pic-test", p, 800).with_out(Box::new(into))),
        |_| {
            // The delete must be written by the frame in which the picture went, while the app
            // is still running: the exit also flushes deletes, and would hide a missing one.
            let seen = sink.clone();
            vec![
                step(placeholder, Some(AppEvent::Resize)),
                step(
                    move |b| sym(b, RECT.0, RECT.1) == "x" && seen.text().contains("a=d"),
                    None,
                ),
            ]
        },
    );
    assert!(out.clears() >= 1, "the old placement stays on screen");
    let text = sink.text();
    assert!(text.contains("a=d"), "{text:?}");
}

#[test]
fn leaving_deletes_every_kitty_image_placed_and_the_panic_helper_does_the_same() {
    let _turn = KITTY.lock().unwrap_or_else(|e| e.into_inner());
    // Exit: the pictures are still on screen when the run ends.
    let sink = Sink::default();
    let into = sink.clone();
    run_pic(
        |_| vec![pic(1, solid(64, 64, [255, 0, 0]))],
        move |p| Some(Pictures::new(kitty_picker(), "pic-test", p, 800).with_out(Box::new(into))),
        |_| {
            let mut not_quit = step(|_| true, None);
            not_quit.act = Some(Box::new(|| panic!("q did not end the run")));
            vec![step(placeholder, Some(key('q'))), not_quit]
        },
    );
    assert!(sink.text().contains("a=d"), "{:?}", sink.text());

    // The panic hook's helper, mid-run: it deletes what is placed, and the exit then has nothing
    // more to delete for it.
    let hook = Sink::default();
    let into_hook = hook.clone();
    let after = Sink::default();
    let into_after = after.clone();
    let mut s = step(placeholder, None);
    s.act = Some(Box::new(move || {
        let mut w = into_hook;
        delete_placed_pictures(&mut w);
    }));
    run_pic(
        |_| vec![pic(1, solid(64, 64, [255, 0, 0]))],
        move |p| {
            Some(Pictures::new(kitty_picker(), "pic-test", p, 800).with_out(Box::new(into_after)))
        },
        |_| vec![s],
    );
    assert!(hook.text().contains("a=d"), "{:?}", hook.text());
    assert_eq!(after.text(), "", "deleted twice");
    // And a table already empty writes nothing.
    let mut again = Vec::new();
    delete_placed_pictures(&mut again);
    assert!(again.is_empty());
}

#[test]
fn a_relative_file_picture_is_read_from_the_directory_of_the_shown_file() {
    let (dir, path) = pic_file();
    let mut img = image::RgbaImage::new(64, 64);
    for p in img.pixels_mut() {
        *p = image::Rgba([0, 255, 0, 255]);
    }
    img.save(dir.as_path().join("g.png")).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = PicPlugin {
        script: Arc::new(|_| {
            vec![
                pic(1, ImageData::File(PathBuf::from("g.png"))),
                CanvasImage::new(
                    2,
                    CellRect::new(10, 1, 4, 2),
                    ImageData::File(PathBuf::from("missing.png")),
                ),
            ]
        }),
        calls,
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(30, 8)));
    let mut term = Terminal::new(Probe {
        inner: Arc::clone(&screen),
        shared: Arc::default(),
        cell: (0, 0),
    })
    .unwrap();
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: vec![step(drawn, None)].into(),
        started: Instant::now(),
        waits: Waits::default(),
    };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx(),
        &AppKeys::defaults(),
        half(&opened.path),
    )
    .unwrap();
    let b = screen.lock().unwrap();
    assert_eq!(pixel(&b.buffer()[(2, 1)]), RColor::Rgb(0, 255, 0));
    // The file that is not there leaves the plugin's cells.
    assert_eq!(b.buffer()[(10, 1)].symbol(), "x");
}

/// An SVG drawer that never finishes until it is told to stop.
struct Stuck {
    started: Arc<AtomicBool>,
    saw_cancel: Arc<AtomicBool>,
}

impl SvgDraw for Stuck {
    fn untrusted(
        &self,
        _: &[u8],
        _: &Path,
        _: Option<&[PathBuf]>,
        _: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.wait(c)
    }
    fn embedded(
        &self,
        _: &[u8],
        _: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.wait(c)
    }
}

impl Stuck {
    fn wait(
        &self,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.started.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !c() {
            assert!(t.elapsed() < DEADLINE, "never cancelled");
            std::thread::sleep(Duration::from_millis(2));
        }
        self.saw_cancel.store(true, Ordering::SeqCst);
        Err(konoma_svg::svg::RasterError::Cancelled)
    }
}

#[test]
fn q_during_a_decode_in_flight_quits_at_once_and_cancels_the_decode() {
    let started = Arc::new(AtomicBool::new(false));
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let (s2, c2) = (Arc::clone(&started), Arc::clone(&saw_cancel));
    let (s3, begun) = (Arc::clone(&started), Instant::now());
    let quit = step(move |_| s3.load(Ordering::SeqCst), Some(key('q')));
    // If `q` did not end the run, the loop asks for another event: this step fails the test.
    let mut not_quit = step(|_| true, None);
    not_quit.act = Some(Box::new(|| panic!("q did not end the run")));
    let out = run_pic(
        |_| vec![pic(1, svg_data(false))],
        move |p| {
            Some(
                Pictures::new(Picker::halfblocks(), "pic-test", p, 800).with_svg(Arc::new(Stuck {
                    started: s2,
                    saw_cancel: c2,
                })),
            )
        },
        |_| vec![quit, not_quit],
    );
    // The run returned while the worker was still inside the drawing (it only stops when told).
    assert!(begun.elapsed() < DEADLINE);
    assert_eq!(out.cell(RECT.0, RECT.1).symbol(), "x", "nothing was drawn");
    // Leaving told the worker to stop.
    let t = Instant::now();
    while !saw_cancel.load(Ordering::SeqCst) {
        assert!(t.elapsed() < DEADLINE, "the decode was never cancelled");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_newer_canvas_cancels_the_decode_in_flight() {
    let started = Arc::new(AtomicBool::new(false));
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let (s2, c2) = (Arc::clone(&started), Arc::clone(&saw_cancel));
    let (s3, c3) = (Arc::clone(&started), Arc::clone(&saw_cancel));
    run_pic(
        |n| {
            if n == 0 {
                vec![pic(1, svg_data(false))]
            } else {
                vec![pic(1, solid(64, 64, [255, 0, 0]))]
            }
        },
        move |p| {
            Some(
                Pictures::new(Picker::halfblocks(), "pic-test", p, 800).with_svg(Arc::new(Stuck {
                    started: s2,
                    saw_cancel: c2,
                })),
            )
        },
        move |_| {
            vec![
                step(move |_| s3.load(Ordering::SeqCst), Some(AppEvent::Resize)),
                step(move |b| c3.load(Ordering::SeqCst) && drawn(b), None),
            ]
        },
    );
}

#[test]
fn the_default_svg_size_is_the_one_konoma_uses() {
    let shared = konoma_host_core::shared::SharedConfig::parse("").expect("parses");
    assert_eq!(
        shared.ui.svg_max_px,
        konoma_plugin::canvas_pictures::DEFAULT_SVG_MAX_PX
    );
    let set = konoma_host_core::shared::SharedConfig::parse("[ui]\nsvg_max_px = 123\n").unwrap();
    assert_eq!(set.ui.svg_max_px, 123);
}

// ---- what the loop is told to wait for ----

/// The longest short poll the app asks for while a decode is out.
const BUSY: Duration = Duration::from_millis(16);

/// An SVG drawer that holds the decode until `release` is set (or the job is cancelled), then
/// draws like the app does.
struct Gated {
    started: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl Gated {
    fn hold(&self, c: &dyn Fn() -> bool) -> Result<(), konoma_svg::svg::RasterError> {
        self.started.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !self.release.load(Ordering::SeqCst) {
            if c() {
                return Err(konoma_svg::svg::RasterError::Cancelled);
            }
            assert!(t.elapsed() < DEADLINE, "never released");
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}

impl SvgDraw for Gated {
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.hold(c)?;
        InProcessSvg.untrusted(data, base, roots, max_px, c)
    }
    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.hold(c)?;
        InProcessSvg.embedded(data, max_px, c)
    }
}

fn gated(p: &Path, started: &Arc<AtomicBool>, release: &Arc<AtomicBool>) -> Option<Pictures> {
    Some(
        Pictures::new(Picker::halfblocks(), "pic-test", p, 800).with_svg(Arc::new(Gated {
            started: Arc::clone(started),
            release: Arc::clone(release),
        })),
    )
}

/// A step that waits for `ready`, then for `calls` more turns of the loop (each asks for a wait),
/// and then runs `act`.
fn after_turns(
    ready: impl Fn(&Buffer) -> bool + 'static,
    calls: usize,
    act: impl FnOnce() + 'static,
    then: Option<AppEvent>,
) -> Step {
    let n = AtomicUsize::new(0);
    let mut s = step(
        move |b| ready(b) && n.fetch_add(1, Ordering::SeqCst) >= calls,
        then,
    );
    s.act = Some(Box::new(act));
    s
}

fn red_svg(b: &Buffer) -> bool {
    let c = &b[(RECT.0, RECT.1)];
    drawn(b) && matches!(pixel(c), RColor::Rgb(r, g, bl) if r > 200 && g < 60 && bl < 60)
}

#[test]
fn with_nothing_out_the_loop_is_not_asked_to_wake_and_while_a_decode_is_out_it_is_asked_briefly() {
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let (s2, r2) = (Arc::clone(&started), Arc::clone(&release));
    let (s3, r3) = (Arc::clone(&started), Arc::clone(&release));
    let held_until = Arc::new(AtomicUsize::new(usize::MAX));
    let (mark, waits_of) = (Arc::clone(&held_until), Arc::new(Mutex::new(None::<Waits>)));
    let w2 = Arc::clone(&waits_of);
    let out = run_pic_waits(
        |n| {
            // Sent once, then `unchanged`: the idle turns must not decode it again.
            if n == 0 {
                vec![pic(1, svg_data(false))]
            } else {
                vec![unchanged(1)]
            }
        },
        move |p| gated(p, &s2, &r2),
        move |_, waits| {
            *w2.lock().unwrap() = Some(Arc::clone(waits));
            let w = Arc::clone(waits);
            let release_it = move || {
                mark.store(w.lock().unwrap().len(), Ordering::SeqCst);
                r3.store(true, Ordering::SeqCst);
            };
            vec![
                // The decode is held for 30 turns of the loop, then let go.
                after_turns(
                    move |_| s3.load(Ordering::SeqCst),
                    30,
                    release_it,
                    Some(AppEvent::Key(Key::char('z'))),
                ),
                // Drawn, and then 20 turns with nothing out.
                after_turns(red_svg, 20, || {}, None),
            ]
        },
    );
    let waits = out.waits.lock().unwrap().clone();
    let held = held_until.load(Ordering::SeqCst);
    assert!(held >= 30 && held < waits.len(), "{held} {}", waits.len());
    // While the decode was out: a short poll, never zero (that would redraw at full speed).
    for (i, w) in waits[..held].iter().enumerate() {
        let d = w.unwrap_or_else(|| panic!("no wake with a decode out (turn {i})"));
        assert!(d > Duration::ZERO && d <= BUSY, "turn {i}: {d:?}");
    }
    // Idle with nothing out: no wake at all.
    let tail = &waits[waits.len() - 10..];
    assert!(tail.iter().all(Option::is_none), "{tail:?}");
}

/// Many pictures (past the decode limit of one answer), so the first answer asks for another.
fn crowd() -> Vec<CanvasImage> {
    (1..=36u64)
        .map(|i| pic(i, solid(2, 2, [(i % 250) as u8 + 1, 0, 0])))
        .collect()
}

#[test]
fn a_decode_held_past_the_follow_up_deadline_does_not_make_the_loop_spin() {
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let (s2, r2) = (Arc::clone(&started), Arc::clone(&release));
    let (s3, r3) = (Arc::clone(&started), Arc::clone(&release));
    let window = Arc::new(Mutex::new((usize::MAX, usize::MAX)));
    let (w_start, w_end) = (Arc::clone(&window), Arc::clone(&window));
    let out = run_pic_waits(
        |n| match n {
            0 => crowd(),
            1 => vec![pic(1, svg_data(false))],
            _ => vec![unchanged(1)],
        },
        move |p| gated(p, &s2, &r2),
        move |_, waits| {
            let (wa, wb) = (Arc::clone(waits), Arc::clone(waits));
            // The first answer landed with pictures left over (the follow-up is due 33ms
            // later). Before the loop looks at the pictures again, a newer canvas comes and
            // starts a job that is held, past that deadline.
            let mut first = step(drawn, Some(AppEvent::Resize));
            first.act = Some(Box::new(move || {
                std::thread::sleep(Duration::from_millis(80));
                w_start.lock().unwrap().0 = wa.lock().unwrap().len();
            }));
            vec![
                first,
                after_turns(
                    move |_| s3.load(Ordering::SeqCst),
                    30,
                    move || {
                        w_end.lock().unwrap().1 = wb.lock().unwrap().len();
                        r3.store(true, Ordering::SeqCst);
                    },
                    None,
                ),
            ]
        },
    );
    let (from, to) = *window.lock().unwrap();
    let waits = out.waits.lock().unwrap().clone();
    assert!(from < to && to <= waits.len(), "{from} {to}");
    for (i, w) in waits[from + 1..to].iter().enumerate() {
        let d = w.unwrap_or_else(|| panic!("no wake with a decode out (turn {i})"));
        assert!(d > Duration::ZERO && d <= BUSY, "turn {i}: {d:?}");
    }
}

fn gif_frames() -> ImageData {
    ImageData::Frames(vec![
        (solid(64, 64, [255, 0, 0]), Duration::from_millis(120)),
        (solid(64, 64, [0, 0, 255]), Duration::from_millis(120)),
    ])
}

/// The canvas of a GIF: sent once, then `unchanged` (so the loop's idle turns do not decode it
/// again).
fn gif_script(n: usize) -> Vec<CanvasImage> {
    if n == 0 {
        vec![pic(1, gif_frames())]
    } else {
        vec![unchanged(1)]
    }
}

fn gif_color(b: &Buffer, c: RColor) -> bool {
    drawn(b) && pixel(&b[(RECT.0, RECT.1)]) == c
}

#[test]
fn a_gif_frame_is_not_changed_before_its_delay_and_the_loop_wakes_by_the_delay() {
    // Each transition is timed from the screen the loop drew. Only a lower bound is asserted
    // (the delay is 120ms; 60ms leaves room for a frame seen late on a loaded machine): a
    // frame that changed early is the failure, being late is not.
    let seen: Arc<Mutex<Vec<(Instant, RColor)>>> = Arc::default();
    let log = Arc::clone(&seen);
    let out = run_pic(gif_script, half, move |_| {
        vec![step(
            move |b| {
                let mut v = log.lock().unwrap();
                for c in [RColor::Rgb(255, 0, 0), RColor::Rgb(0, 0, 255)] {
                    if gif_color(b, c) && v.last().map(|l| l.1) != Some(c) {
                        v.push((Instant::now(), c));
                    }
                }
                // red, blue, red again
                v.len() >= 3
            },
            None,
        )]
    });
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, RColor::Rgb(255, 0, 0));
    assert_eq!(seen[1].1, RColor::Rgb(0, 0, 255));
    for pair in seen.windows(2) {
        let held = pair[1].0.duration_since(pair[0].0);
        assert!(held >= Duration::from_millis(60), "changed after {held:?}");
    }
    // The wakes follow the frame delay: never zero, never past the delay (once the picture is
    // there; before that the decode's short poll).
    let waits = out.waits.lock().unwrap().clone();
    for (i, w) in waits.iter().enumerate() {
        let d = w.unwrap_or_else(|| panic!("no wake while a GIF runs (turn {i})"));
        assert!(
            d >= Duration::from_millis(10) && d <= Duration::from_millis(120),
            "turn {i}: {d:?}"
        );
    }
}

#[test]
fn a_gif_is_not_woken_for_while_the_help_covers_the_canvas() {
    let marks = Arc::new(Mutex::new((usize::MAX, usize::MAX)));
    let (m1, m2) = (Arc::clone(&marks), Arc::clone(&marks));
    let out = run_pic_waits(gif_script, half, move |_, waits| {
        let (w1, w2) = (Arc::clone(waits), Arc::clone(waits));
        let red = |b: &Buffer| gif_color(b, RColor::Rgb(255, 0, 0));
        vec![
            step(red, Some(key('?'))),
            // The help is what the loop has drawn now.
            {
                let mut s = step(|_| true, Some(AppEvent::Resize));
                s.act = Some(Box::new(move || {
                    m1.lock().unwrap().0 = w1.lock().unwrap().len()
                }));
                s
            },
            after_turns(
                |_| true,
                30,
                move || m2.lock().unwrap().1 = w2.lock().unwrap().len(),
                Some(key('?')),
            ),
            // Back on the canvas, 10 more turns.
            after_turns(|_| true, 10, || {}, None),
        ]
    });
    let (from, to) = *marks.lock().unwrap();
    let waits = out.waits.lock().unwrap().clone();
    assert!(from < to && to <= waits.len(), "{from} {to}");
    let covered = &waits[from + 1..to];
    assert!(
        covered.iter().all(Option::is_none),
        "woken for a GIF under the help: {covered:?}"
    );
    // On the canvas again it animates, and the loop is asked to wake for it.
    let after = &waits[to + 3..];
    assert!(
        after
            .iter()
            .all(|w| w.is_some_and(|d| d >= Duration::from_millis(10))),
        "{after:?}"
    );
}

// ---- a document that is reopened as another ----

/// Shows a document with one local picture the first time, then answers a `Reopen` as a
/// document of text only.
struct DocConv {
    md: String,
}

impl Plugin for DocConv {
    fn manifest(&self) -> Manifest {
        Manifest::new("doc-conv", "0.0.1")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Document)
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(DocConvSession {
            md: self.md.clone(),
            asked: 0,
        }))
    }
}

struct DocConvSession {
    md: String,
    asked: usize,
}

fn text_doc(markdown: String) -> konoma_plugin::view::DocumentView {
    konoma_plugin::view::DocumentView {
        markdown,
        origin: konoma_plugin::input::Origin::File(PathBuf::from("x.md")),
        resources: Default::default(),
        options: Default::default(),
        title: None,
        truncated: false,
    }
}

impl Session for DocConvSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.asked += 1;
        if self.asked == 1 {
            return Ok(View::Document(text_doc(self.md.clone())));
        }
        Ok(View::Reopen(konoma_plugin::view::Reopen {
            content_type: "markdown".into(),
            text: "reopened text\n".into(),
            resources: Default::default(),
            origin: konoma_plugin::input::Origin::Converted {
                from: PathBuf::from("a.pic"),
            },
            truncated: false,
        }))
    }
}

struct TextTarget;

impl Plugin for TextTarget {
    fn manifest(&self) -> Manifest {
        Manifest::new("text-target", "0.0.1")
            .bind(Bind::content("markdown"))
            .shows(ViewKind::Document)
    }
    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Content { text, .. } = input else {
            return Err(Failure::unsupported(t!(en: "content only")));
        };
        struct S(String);
        impl Session for S {
            fn view(&mut self, _r: &ViewReq, _c: &ViewCx) -> Result<View, Failure> {
                Ok(View::Document(text_doc(self.0.clone())))
            }
        }
        Ok(Box::new(S(text)))
    }
}

#[test]
fn a_reopen_deletes_the_kitty_images_the_old_document_had_placed() {
    let _turn = KITTY.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = pic_file();
    let mut img = image::RgbaImage::new(64, 64);
    for p in img.pixels_mut() {
        *p = image::Rgba([0, 255, 0, 255]);
    }
    let png = dir.as_path().join("g.png");
    img.save(&png).unwrap();
    let plugin = DocConv {
        md: format!("![g]({})\n\ntext\n", png.display()),
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(40, 12)));
    let mut term = Terminal::new(Probe {
        inner: Arc::clone(&screen),
        shared: Arc::default(),
        cell: (0, 0),
    })
    .unwrap();
    let sink = Sink::default();
    let into = sink.clone();
    let seen = sink.clone();
    let any_placeholder = |b: &Buffer| b.content().iter().any(|c| c.symbol().contains(PLACEHOLDER));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: vec![
            step(any_placeholder, Some(AppEvent::Resize)),
            // The delete must come from the frames after the reopen, while the app runs (the
            // exit also deletes what is placed, and would hide a missing one).
            step(
                move |b| {
                    seen.text().contains("a=d")
                        && b.content()
                            .iter()
                            .all(|c| !c.symbol().contains(PLACEHOLDER))
                        && (0..b.area.height).any(|y| {
                            (0..b.area.width)
                                .map(|x| b[(x, y)].symbol().to_string())
                                .collect::<String>()
                                .contains("reopened text")
                        })
                },
                None,
            ),
        ]
        .into(),
        started: Instant::now(),
        waits: Waits::default(),
    };
    let target: Arc<dyn Plugin> = Arc::new(TextTarget);
    let cx = AppCx {
        reopen: Some(konoma_plugin::app::ReopenSource::new(move |_| {
            Some(konoma_plugin::reopen::ContentTarget {
                plugin: Arc::clone(&target),
                settings: Settings::empty().clone(),
            })
        })),
        ..cx()
    };
    let pictures = Pictures::new(kitty_picker(), "pic-test", &opened.path, 800)
        .with_out(Box::new(into))
        .with_documents(konoma_plugin::app::DocEnv::default());
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        Some(pictures),
    )
    .expect("runs");
}

// ---- an embedded render in flight when the document goes away ----

/// An embed plugin (the bundled kind: its SVG is drawn in this process) that holds every render
/// until `open` is set, and says when one has begun and when it is done.
struct HeldEmbed {
    kind: &'static str,
    begun: Arc<AtomicBool>,
    open: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Plugin for HeldEmbed {
    fn manifest(&self) -> Manifest {
        Manifest::new("held-embed", "0.0.1").bind(Bind::embed(self.kind))
    }
    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if !matches!(input, Input::Embed { .. }) {
            return Err(Failure::unsupported(t!(en: "embed only")));
        }
        self.begun.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !self.open.load(Ordering::SeqCst) {
            assert!(t.elapsed() < DEADLINE, "the render was never released");
            std::thread::sleep(Duration::from_millis(2));
        }
        self.done.store(true, Ordering::SeqCst);
        struct S;
        impl Session for S {
            fn view(&mut self, _r: &ViewReq, _c: &ViewCx) -> Result<View, Failure> {
                Ok(View::Image(konoma_plugin::view::ImageView::new(
                    ImageData::Svg {
                        data: Arc::from(RED_SVG.as_bytes()),
                        base_dir: None,
                        trusted: true,
                    },
                )))
            }
        }
        Ok(Box::new(S))
    }
}

struct Held {
    begun: Arc<AtomicBool>,
    open: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    env: konoma_plugin::app::DocEnv,
}

fn held_env(kind: &'static str) -> Held {
    let flag = || Arc::new(AtomicBool::new(false));
    let (begun, open, done) = (flag(), flag(), flag());
    let mut registry = konoma_plugin::registry::Registry::new();
    registry
        .register(
            Arc::new(HeldEmbed {
                kind,
                begun: Arc::clone(&begun),
                open: Arc::clone(&open),
                done: Arc::clone(&done),
            }),
            konoma_plugin::registry::PluginOrigin::Bundled,
        )
        .expect("registers");
    let env = konoma_plugin::app::DocEnv::from_registry(
        Arc::new(registry),
        "en",
        Box::new(|_| Settings::empty().clone()),
        false,
    );
    Held {
        begun,
        open,
        done,
        env,
    }
}

fn shows(b: &Buffer, needle: &str) -> bool {
    (0..b.area.height).any(|y| {
        (0..b.area.width)
            .map(|x| b[(x, y)].symbol().to_string())
            .collect::<String>()
            .contains(needle)
    })
}

/// Reopening while the embedded render is in flight: the old document's late result lands on
/// nothing, so no picture is placed or transmitted afterwards.
fn reopen_during_embed(kind: &'static str, md: &str) {
    let _turn = KITTY.lock().unwrap_or_else(|e| e.into_inner());
    let (_dir, path) = pic_file();
    let held = held_env(kind);
    let mut opened = open_file(
        vec![Arc::new(DocConv { md: md.to_string() })],
        &path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(40, 12)));
    let mut term = Terminal::new(Probe {
        inner: Arc::clone(&screen),
        shared: Arc::default(),
        cell: (0, 0),
    })
    .unwrap();
    let sink = Sink::default();
    let into = sink.clone();
    let begun = Arc::clone(&held.begun);
    let (open, done) = (Arc::clone(&held.open), Arc::clone(&held.done));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: vec![
            // The render has begun and is held: the document goes away under it.
            step(
                move |_| begun.load(Ordering::SeqCst),
                Some(AppEvent::Resize),
            ),
            {
                let mut s = step(|b| shows(b, "reopened text"), Some(AppEvent::Resize));
                s.act = Some(Box::new(move || open.store(true, Ordering::SeqCst)));
                s
            },
            // The old render has finished and had its frames to land in.
            step(move |_| done.load(Ordering::SeqCst), Some(AppEvent::Resize)),
            step(
                |b| {
                    shows(b, "reopened text")
                        && b.content()
                            .iter()
                            .all(|c| !c.symbol().contains(PLACEHOLDER))
                },
                None,
            ),
        ]
        .into(),
        started: Instant::now(),
        waits: Waits::default(),
    };
    let target: Arc<dyn Plugin> = Arc::new(TextTarget);
    let cx = AppCx {
        reopen: Some(konoma_plugin::app::ReopenSource::new(move |_| {
            Some(konoma_plugin::reopen::ContentTarget {
                plugin: Arc::clone(&target),
                settings: Settings::empty().clone(),
            })
        })),
        ..cx()
    };
    let pictures = Pictures::new(kitty_picker(), "pic-test", &opened.path, 800)
        .with_out(Box::new(into))
        .with_documents(held.env);
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        Some(pictures),
    )
    .expect("runs");
    assert!(
        !sink.text().contains("a=T"),
        "a picture of the old document was transmitted"
    );
}

#[test]
fn a_reopen_during_a_math_render_leaves_no_picture() {
    reopen_during_embed("math", "before $x^2$ after\n");
}

#[test]
fn a_reopen_during_a_mermaid_render_leaves_no_picture() {
    reopen_during_embed("fence:mermaid", "```mermaid\ngraph TD; A-->B\n```\n");
}

/// Quitting while the embedded render is in flight returns at once, and the render that ends
/// afterwards places nothing.
fn quit_during_embed(kind: &'static str, md: &str) {
    let _turn = KITTY.lock().unwrap_or_else(|e| e.into_inner());
    let (_dir, path) = pic_file();
    let held = held_env(kind);
    let mut opened = open_file(
        vec![Arc::new(DocConv { md: md.to_string() })],
        &path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(40, 12)));
    let mut term = Terminal::new(Probe {
        inner: Arc::clone(&screen),
        shared: Arc::default(),
        cell: (0, 0),
    })
    .unwrap();
    let sink = Sink::default();
    let into = sink.clone();
    let begun = Arc::clone(&held.begun);
    let mut not_quit = step(|_| true, None);
    not_quit.act = Some(Box::new(|| panic!("q did not end the run")));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: vec![
            step(move |_| begun.load(Ordering::SeqCst), Some(key('q'))),
            not_quit,
        ]
        .into(),
        started: Instant::now(),
        waits: Waits::default(),
    };
    let pictures = Pictures::new(kitty_picker(), "pic-test", &opened.path, 800)
        .with_out(Box::new(into))
        .with_documents(held.env);
    let begun_at = Instant::now();
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx(),
        &AppKeys::defaults(),
        Some(pictures),
    )
    .expect("runs");
    // The run ended while the render was still held (it only ends when released).
    assert!(begun_at.elapsed() < DEADLINE);
    assert!(
        !held.done.load(Ordering::SeqCst),
        "the render was not in flight when the run ended"
    );
    held.open.store(true, Ordering::SeqCst);
    let t = Instant::now();
    while !held.done.load(Ordering::SeqCst) {
        assert!(t.elapsed() < DEADLINE, "the worker never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !sink.text().contains("a=T"),
        "a picture was transmitted after the quit"
    );
}

#[test]
fn quitting_during_a_math_render_returns_at_once_and_places_nothing() {
    quit_during_embed("math", "before $x^2$ after\n");
}

#[test]
fn quitting_during_a_mermaid_render_returns_at_once_and_places_nothing() {
    quit_during_embed("fence:mermaid", "```mermaid\ngraph TD; A-->B\n```\n");
}
