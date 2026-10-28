//! The still picture of a plugin's `View::Image` in the standalone app: decoded on a thread and
//! drawn through the shared viewer on a `TestBackend` with a halfblocks picker (never a real
//! terminal), zoomed and panned with the keys, and worded like konoma when it cannot be shown.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::app::{
    open_file, run_with_pictures, AppCx, AppEvent, AppKeys, EventSource, InProcessSvg, Next,
    Pictures,
};
use konoma_plugin::canvas_pictures::SvgDraw;
use konoma_plugin::prelude::*;
use konoma_plugin::style::Line;
use konoma_plugin::view::{ImageData, ImageView, MessageView};
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::Color as RColor;
use ratatui::Terminal;
use ratatui_image::picker::Picker;

/// The longest any test waits for a state that must come (a failure bound, never an expectation).
const DEADLINE: Duration = Duration::from_secs(20);
const W: u16 = 100;
const H: u16 = 14;

// ---- the plugin: a picture view made by a script of the call number ----

type Script = Arc<dyn Fn(usize, &Path) -> View + Send + Sync>;

struct ImgPlugin {
    script: Script,
    calls: Arc<AtomicUsize>,
    dir: PathBuf,
}

impl Plugin for ImgPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("img-test", "0.0.1")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Image)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ImgSession {
            script: Arc::clone(&self.script),
            calls: Arc::clone(&self.calls),
            dir: self.dir.clone(),
        }))
    }
}

struct ImgSession {
    script: Script,
    calls: Arc<AtomicUsize>,
    dir: PathBuf,
}

impl Session for ImgSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.script)(n, &self.dir))
    }
}

// ---- the backend: a TestBackend whose buffer the event source can look at ----

struct Shared(Arc<Mutex<TestBackend>>);

impl Backend for Shared {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.lock().unwrap().draw(content)
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
        self.0.lock().unwrap().clear()
    }
    fn clear_region(&mut self, t: ClearType) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear_region(t)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.0.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ---- the events: wait for a state of the screen, then do the next thing ----

struct Step {
    until: Box<dyn FnMut(&Buffer) -> bool>,
    /// Run when the state came, before `then`.
    act: Option<Box<dyn FnOnce()>>,
    /// `None` ends the run.
    then: Option<AppEvent>,
}

fn step(until: impl FnMut(&Buffer) -> bool + 'static, then: Option<AppEvent>) -> Step {
    Step {
        until: Box::new(until),
        act: None,
        then,
    }
}

struct Driver {
    screen: Arc<Mutex<TestBackend>>,
    steps: VecDeque<Step>,
    started: Instant,
    log: Vec<String>,
}

impl EventSource for Driver {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        loop {
            let Some(front) = self.steps.front_mut() else {
                return Next::End;
            };
            let ready = (front.until)(self.screen.lock().unwrap().buffer());
            {
                let b = self.screen.lock().unwrap();
                let n = inner(b.buffer()).iter().filter(|c| is_red(c)).count();
                let line = format!("{n} {}", footer(b.buffer()));
                if self.log.last() != Some(&line) {
                    self.log.push(line);
                }
            }
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
                let cols: std::collections::BTreeMap<String, usize> = inner(b.buffer())
                    .iter()
                    .fold(Default::default(), |mut m, c| {
                        *m.entry(format!("{:?}/{:?}/{}", c.1, c.2, c.0)).or_default() += 1;
                        m
                    });
                panic!("the screen never got there: {cols:?} {:#?}", self.log);
            }
            // A real source returns only when its wait is up; with none it blocks until an event.
            // So the loop is asked for a new view on a timeout only when it asked to wake.
            match wait {
                Some(w) => {
                    std::thread::sleep(w.min(Duration::from_millis(3)));
                    return Next::Timeout;
                }
                None => std::thread::sleep(Duration::from_millis(3)),
            }
        }
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
    }
}

fn rows(b: &Buffer) -> Vec<String> {
    (0..b.area.height)
        .map(|y| {
            (0..b.area.width)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn footer(b: &Buffer) -> String {
    rows(b).pop().unwrap_or_default()
}

fn shows(b: &Buffer, text: &str) -> bool {
    rows(b).iter().any(|r| r.contains(text))
}

/// The cells inside the border of the frame (its title row and the footer row left out).
fn inner(b: &Buffer) -> Vec<(String, RColor, RColor)> {
    let mut out = Vec::new();
    for y in 2..b.area.height.saturating_sub(2) {
        for x in 1..b.area.width.saturating_sub(1) {
            let c = &b[(x, y)];
            out.push((c.symbol().to_string(), c.fg, c.bg));
        }
    }
    out
}

/// A cell of the picture: the upper half block has the picture's pixel as its color, and a cell
/// of two equal halves is a space on that color.
fn is_red(c: &(String, RColor, RColor)) -> bool {
    let red = |col: &RColor| matches!(col, RColor::Rgb(r, g, b) if *r > 200 && *g < 60 && *b < 60);
    if c.0 == " " {
        red(&c.2)
    } else {
        red(&c.1) || red(&c.2)
    }
}

/// The picture is on screen: red cells inside the frame (the loading line and the failure line
/// are text with no color).
fn has_red(b: &Buffer) -> bool {
    inner(b).iter().filter(|c| is_red(c)).count() >= 8
}

fn png_file(dir: &Path, name: &str, img: &image::RgbaImage) -> PathBuf {
    let p = dir.join(name);
    img.save(&p).expect("a png is written");
    p
}

fn red_image() -> image::RgbaImage {
    image::RgbaImage::from_pixel(160, 160, image::Rgba([255, 0, 0, 255]))
}

/// Diagonal bands of four colors, red first, so a pan in either direction shows other colors.
fn stripes() -> image::RgbaImage {
    const COLORS: [[u8; 3]; 4] = [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 0]];
    image::RgbaImage::from_fn(320, 320, |x, y| {
        let c = COLORS[(x / 40 + y / 40) as usize % COLORS.len()];
        image::Rgba([c[0], c[1], c[2], 255])
    })
}

fn file_view(p: &Path) -> View {
    View::Image(ImageView::new(ImageData::File(p.to_path_buf())))
}

fn half(path: &Path) -> Option<Pictures> {
    Some(Pictures::new(Picker::halfblocks(), "img-test", path, 800))
}

/// What a run left behind.
struct Out {
    screen: Arc<Mutex<TestBackend>>,
    calls: Arc<AtomicUsize>,
}

impl Out {
    fn rows(&self) -> Vec<String> {
        rows(self.screen.lock().unwrap().buffer())
    }
}

/// Run the app over the picture views `script` makes (called with the call number and a
/// directory that lives as long as the run), with `pictures` (made from the opened file's path).
fn run_img(
    script: impl Fn(usize, &Path) -> View + Send + Sync + 'static,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: impl FnOnce(&Arc<AtomicUsize>) -> Vec<Step>,
) -> Out {
    let dir = unique_tmp("app_still");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let path = dir.as_path().join("a.pic");
    std::fs::write(&path, "x").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = ImgPlugin {
        script: Arc::new(script),
        calls: Arc::clone(&calls),
        dir: dir.as_path().to_path_buf(),
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: steps(&calls).into(),
        started: Instant::now(),
        log: Vec::new(),
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
    drop(dir);
    Out { screen, calls }
}

/// A picture file made in the run's directory, then shown.
fn png_script(
    make: fn() -> image::RgbaImage,
) -> impl Fn(usize, &Path) -> View + Send + Sync + 'static {
    move |_, dir| file_view(&png_file(dir, "p.png", &make()))
}

// ---- drawing: loading, then the picture ----

#[test]
fn a_png_is_decoded_off_the_loop_and_drawn_inside_the_frame_with_its_zoom_in_the_footer() {
    let out = run_img(png_script(red_image), half, |_| {
        vec![step(|b| has_red(b) && footer(b).contains("x1.00"), None)]
    });
    let r = out.rows();
    assert!(r[0].starts_with('┌') && r[0].contains(" title "), "{r:?}");
    assert!(r[r.len() - 2].starts_with('└'), "{r:?}");
    assert!(!r.iter().any(|l| l.contains("loading")), "{r:?}");
    // The picture is inside the border and nowhere else.
    let b = out.screen.lock().unwrap();
    let b = b.buffer();
    for x in [0, b.area.width - 1] {
        for y in 1..b.area.height - 2 {
            assert!(
                !matches!(b[(x, y)].bg, RColor::Rgb(..)),
                "the border column {x} row {y} was painted over"
            );
        }
    }
    assert_eq!(out.calls.load(Ordering::SeqCst), 1, "one view, no re-asks");
}

const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#ff0000"/></svg>"##;

/// An SVG drawer that holds the decode until `release` is set (or the job is cancelled), then
/// draws like the app does.
struct Gated {
    release: Arc<AtomicBool>,
}

impl Gated {
    fn hold(&self, c: &dyn Fn() -> bool) -> Result<(), konoma_svg::svg::RasterError> {
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

fn svg_view() -> View {
    View::Image(ImageView::new(ImageData::Svg {
        data: Arc::from(RED_SVG.as_bytes()),
        base_dir: None,
        trusted: false,
    }))
}

#[test]
fn an_svg_shows_the_spinner_and_loading_until_it_is_drawn_then_the_picture() {
    let release = Arc::new(AtomicBool::new(false));
    let (r_pictures, r_step) = (Arc::clone(&release), Arc::clone(&release));
    let mut loading = step(
        |b| {
            // The spinner glyph (a braille pattern), two spaces, the word, centred in the frame.
            let line = rows(b).into_iter().find(|l| l.contains("  loading…"));
            let Some(line) = line else { return false };
            assert!(
                line.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
                "{line:?}"
            );
            assert!(!has_red(b), "nothing is drawn while it loads");
            true
        },
        Some(key('x')),
    );
    loading.act = Some(Box::new(move || r_step.store(true, Ordering::SeqCst)));
    let out = run_img(
        |_, _| svg_view(),
        move |p| {
            Some(
                Pictures::new(Picker::halfblocks(), "img-test", p, 800).with_svg(Arc::new(Gated {
                    release: r_pictures,
                })),
            )
        },
        |_| vec![loading, step(|b| has_red(b) && !shows(b, "loading"), None)],
    );
    assert!(release.load(Ordering::SeqCst));
    assert!(out.rows()[0].contains(" title "));
}

// ---- failures: konoma's words ----

fn failure_run(
    script: impl Fn(usize, &Path) -> View + Send + Sync + 'static,
    pictures: bool,
) -> Vec<String> {
    let out = run_img(
        script,
        move |p| if pictures { half(p) } else { None },
        |_| {
            vec![step(
                |b| shows(b, "[image]") || shows(b, "unsupported"),
                None,
            )]
        },
    );
    out.rows()
}

fn line_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle:?} in {rows:#?}"))
}

/// The text of a row inside the border.
fn framed(row: &str) -> String {
    row.trim_start_matches('│')
        .trim_end_matches('│')
        .trim()
        .to_string()
}

/// A PNG of a few dozen bytes whose header announces `w` x `h` pixels (a real 1-pixel picture
/// with the size fields patched; the pixel data is not there to decode).
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

#[test]
fn a_picture_that_cannot_be_shown_says_why_in_konomas_words() {
    let mut said = 0;
    // The file is not an image at all.
    let r = failure_run(
        |_, dir| {
            let p = dir.join("n.dat");
            std::fs::write(&p, b"this is not a picture").unwrap();
            file_view(&p)
        },
        true,
    );
    assert_eq!(
        framed(line_with(&r, "[image]")),
        "[image] unsupported image format"
    );
    said += 1;
    // A real header, cut short.
    let r = failure_run(
        |_, dir| {
            let p = png_file(dir, "ok.png", &red_image());
            let bytes = std::fs::read(&p).unwrap();
            std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
            file_view(&p)
        },
        true,
    );
    assert_eq!(
        framed(line_with(&r, "[image]")),
        "[image] the file is damaged or could not be read"
    );
    said += 1;
    // A header that announces more pixels than the limit.
    let r = failure_run(
        |_, dir| {
            let p = dir.join("big.png");
            std::fs::write(&p, huge_png(40_000, 40_000)).unwrap();
            file_view(&p)
        },
        true,
    );
    assert_eq!(
        framed(line_with(&r, "[image]")),
        "[image] too large to display (limits: 32,768 px per side, 150 megapixels, 512 MiB decoded)"
    );
    said += 1;
    // A file that is not there.
    let r = failure_run(|_, dir| file_view(&dir.join("missing.png")), true);
    assert!(framed(line_with(&r, "[image]")).starts_with("[image] "));
    said += 1;
    // A broken SVG.
    let r = failure_run(
        |_, _| {
            View::Image(ImageView::new(ImageData::Svg {
                data: Arc::from(&b"<svg"[..]),
                base_dir: None,
                trusted: false,
            }))
        },
        true,
    );
    assert_eq!(
        framed(line_with(&r, "[image]")),
        "[image] [svg] not drawn: the file is damaged or is not a valid SVG"
    );
    said += 1;
    assert_eq!(said, 5);
}

#[test]
fn with_no_pictures_the_terminal_is_named_as_the_reason() {
    let r = failure_run(png_script(red_image), false);
    assert_eq!(
        framed(line_with(&r, "[image]")),
        "[image] cannot display image in this terminal, or failed to load"
    );
}

// ---- zoom and pan ----

#[test]
fn plus_and_minus_zoom_by_a_quarter_step_and_the_footer_says_how_far() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let note = |seen: &Arc<Mutex<Vec<String>>>, zoom: &'static str| {
        let seen = Arc::clone(seen);
        move |b: &Buffer| {
            let f = footer(b);
            if has_red(b) && f.contains(zoom) {
                seen.lock().unwrap().push(zoom.to_string());
                true
            } else {
                false
            }
        }
    };
    let out = run_img(png_script(stripes), half, |_| {
        vec![
            step(note(&seen, "x1.00"), Some(key('-'))),
            // `-` at fit stays at fit; `+` then goes up by a quarter each press.
            step(note(&seen, "x1.00"), Some(key('+'))),
            step(note(&seen, "x1.25"), Some(key('+'))),
            step(note(&seen, "x1.56"), Some(key('-'))),
            step(note(&seen, "x1.25"), Some(key('+'))),
            step(note(&seen, "x1.56"), Some(key('='))),
            step(note(&seen, "x1.00"), None),
        ]
    });
    assert_eq!(
        *seen.lock().unwrap(),
        ["x1.00", "x1.00", "x1.25", "x1.56", "x1.25", "x1.56", "x1.00"]
    );
    assert_eq!(
        out.calls.load(Ordering::SeqCst),
        1,
        "no key asked the session again"
    );
}

/// The picture the loop last drew, kept when the step runs.
type Snap = Arc<Mutex<Vec<(String, RColor, RColor)>>>;

fn snapshot(snap: &Snap, until: impl Fn(&Buffer) -> bool + 'static) -> impl FnMut(&Buffer) -> bool {
    let snap = Arc::clone(snap);
    move |b| {
        if until(b) {
            *snap.lock().unwrap() = inner(b);
            true
        } else {
            false
        }
    }
}

#[test]
fn the_pan_keys_move_a_zoomed_picture_along_the_clipped_axis_and_do_nothing_at_fit() {
    let snap: Snap = Snap::default();
    // After a key the loop needs a few turns before a change could show: wait for them.
    let turns = Arc::new(AtomicUsize::new(0));
    let t2 = Arc::clone(&turns);
    let unchanged = Arc::new(AtomicBool::new(false));
    let u2 = Arc::clone(&unchanged);
    let s1 = Arc::clone(&snap);
    let s2 = Arc::clone(&snap);
    let s3 = Arc::clone(&snap);
    let steps = move |_: &Arc<AtomicUsize>| {
        vec![
            // At fit: pan keys change nothing, however long the loop turns.
            step(
                snapshot(&s1, |b| has_red(b) && footer(b).contains("x1.00")),
                Some(key('j')),
            ),
            step(
                move |b| {
                    if t2.fetch_add(1, Ordering::SeqCst) < 40 {
                        return false;
                    }
                    assert_eq!(
                        inner(b),
                        *s2.lock().unwrap(),
                        "a pan at fit moved the picture"
                    );
                    u2.store(true, Ordering::SeqCst);
                    true
                },
                Some(key('+')),
            ),
            step(|b| footer(b).contains("x1.25"), Some(key('+'))),
            step(|b| footer(b).contains("x1.56"), Some(key('+'))),
            step(
                snapshot(&s3, |b| has_red(b) && footer(b).contains("x1.95")),
                Some(key('j')),
            ),
            // Zoomed: the picture moves (down: the axis that is clipped at this size).
            {
                let s4 = Arc::clone(&snap);
                step(move |b| has_red(b) && inner(b) != *s4.lock().unwrap(), None)
            },
        ]
    };
    run_img(png_script(stripes), half, steps);
    assert!(unchanged.load(Ordering::SeqCst));
}

// ---- the held picture ----

/// A small picture shown whole in a large area: a zoom changes the cells it is drawn in but not
/// the part of the picture that is cut out, and the resized picture must still come to the screen
/// with no key to wake the loop.
#[test]
fn a_zoom_that_only_changes_the_size_still_redraws_the_picture_without_another_event() {
    run_img(png_script(red_image), half, |_| {
        let red_cells = |b: &Buffer| inner(b).iter().filter(|c| is_red(c)).count();
        let before = Arc::new(AtomicUsize::new(0));
        let (b1, b2) = (Arc::clone(&before), Arc::clone(&before));
        vec![
            step(
                move |b| {
                    let n = red_cells(b);
                    b1.store(n, Ordering::SeqCst);
                    n >= 8 && footer(b).contains("x1.00")
                },
                Some(key('+')),
            ),
            // Bigger than at fit: more cells of the picture, at the new zoom.
            step(
                move |b| footer(b).contains("x1.25") && red_cells(b) > b2.load(Ordering::SeqCst),
                None,
            ),
        ]
    });
}

#[test]
fn a_view_that_comes_again_unchanged_keeps_the_picture_and_its_zoom() {
    let out = run_img(png_script(red_image), half, |calls| {
        let c = Arc::clone(calls);
        vec![
            step(
                |b| has_red(b) && footer(b).contains("x1.00"),
                Some(key('+')),
            ),
            step(
                |b| has_red(b) && footer(b).contains("x1.25"),
                Some(AppEvent::Resize),
            ),
            // The resize asked the session again, and it said the same picture.
            step(
                move |b| {
                    if c.load(Ordering::SeqCst) < 2 {
                        return false;
                    }
                    assert!(!shows(b, "loading"), "the same picture was decoded again");
                    has_red(b) && footer(b).contains("x1.25")
                },
                None,
            ),
        ]
    });
    assert!(out.calls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn a_view_that_is_not_a_picture_any_more_leaves_no_picture_behind() {
    run_img(
        |n, dir| match n {
            1 => View::Message(MessageView {
                lines: vec![Line::raw("only text now")],
            }),
            _ => file_view(&png_file(dir, "p.png", &red_image())),
        },
        half,
        |_| {
            vec![
                step(has_red, Some(AppEvent::Resize)),
                step(
                    |b| {
                        let text = shows(b, "only text now");
                        if text {
                            assert!(!has_red(b), "the old picture is still on screen");
                            assert!(!footer(b).contains("x1"), "{:?}", footer(b));
                        }
                        text
                    },
                    Some(AppEvent::Resize),
                ),
                // Back to a picture: it is decoded and drawn again from nothing.
                step(|b| has_red(b) && !shows(b, "only text now"), None),
            ]
        },
    );
}
