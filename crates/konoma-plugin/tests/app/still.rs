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

/// The waits the loop asked for, each with the color on screen at the time.
type Waits = Arc<Mutex<Vec<(Option<Duration>, char)>>>;

struct Driver {
    screen: Arc<Mutex<TestBackend>>,
    steps: VecDeque<Step>,
    started: Instant,
    log: Vec<String>,
    /// Every wait the loop asked for, in order, with the color on screen then (r, b, g or `-`).
    waits: Waits,
}

impl EventSource for Driver {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        {
            let b = self.screen.lock().unwrap();
            let on = if has_red(b.buffer()) {
                'r'
            } else if has_blue(b.buffer()) {
                'b'
            } else if has_green(b.buffer()) {
                'g'
            } else {
                '-'
            };
            self.waits.lock().unwrap().push((wait, on));
        }
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
                panic!(
                    "the screen never got there: {cols:?} {:#?}\n{}",
                    self.log,
                    rows(b.buffer()).join("\n")
                );
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
        ..AppCx::default()
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

fn code(k: KeyCode) -> AppEvent {
    AppEvent::Key(Key::new(k))
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
    waits: Waits,
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
    run_img_in(cx(), script, pictures, steps)
}

/// [`run_img`] with the app's context `cx` (what the plugin declares for a picture that is not
/// there, say).
fn run_img_in(
    cx: AppCx,
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
    let waits = Arc::new(Mutex::new(Vec::new()));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: steps(&calls).into(),
        started: Instant::now(),
        log: Vec::new(),
        waits: Arc::clone(&waits),
    };
    let pictures = pictures(&opened.path);
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        pictures,
    )
    .expect("runs");
    drop(dir);
    Out {
        screen,
        calls,
        waits,
    }
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
    // The `image` plugin names the picture's own frame for a picture that is not there.
    let picture_plugin = AppCx {
        unavailable: konoma_plugin::Unavailable::Picture,
        ..cx()
    };
    let out = run_img_in(
        picture_plugin,
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

// ---- pages: a picture with several pages is turned with J / K ----

fn is_blue(c: &(String, RColor, RColor)) -> bool {
    let blue = |col: &RColor| matches!(col, RColor::Rgb(r, g, b) if *r < 60 && *g < 60 && *b > 200);
    if c.0 == " " {
        blue(&c.2)
    } else {
        blue(&c.1) || blue(&c.2)
    }
}

fn has_blue(b: &Buffer) -> bool {
    inner(b).iter().filter(|c| is_blue(c)).count() >= 8
}

fn is_green(c: &(String, RColor, RColor)) -> bool {
    let g = |col: &RColor| matches!(col, RColor::Rgb(r, g, b) if *r < 60 && *g > 200 && *b < 60);
    g(&c.1) || g(&c.2)
}

fn has_green(b: &Buffer) -> bool {
    inner(b).iter().filter(|c| is_green(c)).count() >= 8
}

/// Page 1 of the svg pages: red on the top half, yellow on the bottom, so a zoom or a pan that
/// moves the window shows in the colors on screen.
const TWO_TONE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="20" fill="#ff0000"/><rect y="20" width="40" height="20" fill="#ffff00"/></svg>"##;
const BLUE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#0000ff"/></svg>"##;
const GREEN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#00ff00"/></svg>"##;

/// An `SvgDraw` that holds the decode of the blue page until `release` is set (it gives up when
/// it is cancelled), so a page turn can be looked at while its picture is on its way.
struct BlueHeld(Arc<AtomicBool>);

impl BlueHeld {
    fn hold(&self, data: &[u8], c: &dyn Fn() -> bool) -> Result<(), konoma_svg::svg::RasterError> {
        if !String::from_utf8_lossy(data).contains("0000ff") {
            return Ok(());
        }
        let t = Instant::now();
        while !self.0.load(Ordering::SeqCst) {
            if c() {
                return Err(konoma_svg::svg::RasterError::Cancelled);
            }
            assert!(t.elapsed() < DEADLINE, "never released");
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}

impl SvgDraw for BlueHeld {
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.hold(data, c)?;
        InProcessSvg.untrusted(data, base, roots, max_px, c)
    }
    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        c: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.hold(data, c)?;
        InProcessSvg.embedded(data, max_px, c)
    }
}

/// `Pictures` whose blue page is held until `release`.
fn blue_held(release: &Arc<AtomicBool>) -> impl FnOnce(&Path) -> Option<Pictures> {
    let release = Arc::clone(release);
    move |p| {
        Some(
            Pictures::new(Picker::halfblocks(), "img-test", p, 800)
                .with_svg(Arc::new(BlueHeld(release))),
        )
    }
}

fn is_yellow(c: &(String, RColor, RColor)) -> bool {
    let y = |col: &RColor| matches!(col, RColor::Rgb(r, g, b) if *r > 200 && *g > 200 && *b < 60);
    y(&c.1) || y(&c.2)
}

/// A cell the picture painted (a color of its own, not the terminal's).
fn is_picture(c: &(String, RColor, RColor)) -> bool {
    matches!(c.1, RColor::Rgb(..)) || matches!(c.2, RColor::Rgb(..))
}

/// The cells of the picture in the color `is` says.
fn cells(b: &Buffer, is: fn(&(String, RColor, RColor)) -> bool) -> usize {
    inner(b).iter().filter(|c| is(c)).count()
}

/// A picture with `count` pages (red, blue, green, red, ...) that records the page of every
/// `view` it is asked for. With `svg` the pages are SVGs, red or blue, so a decode can be held.
struct PagedPlugin {
    count: usize,
    asked: Arc<Mutex<Vec<Option<usize>>>>,
    dir: PathBuf,
    svg: bool,
    /// Page 0 is a two-frame animation, red then green (the other pages are as without it).
    gif: bool,
    /// The page (from 0) whose `view` fails.
    fail: Option<usize>,
}

struct PagedSession {
    count: usize,
    asked: Arc<Mutex<Vec<Option<usize>>>>,
    dir: PathBuf,
    svg: bool,
    gif: bool,
    fail: Option<usize>,
}

impl Plugin for PagedPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("img-test", "0.0.1")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Image)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(PagedSession {
            count: self.count,
            asked: Arc::clone(&self.asked),
            dir: self.dir.clone(),
            svg: self.svg,
            gif: self.gif,
            fail: self.fail,
        }))
    }
}

impl Session for PagedSession {
    fn layout(&self) -> Layout {
        Layout {
            pages: Some(PageLayout::Separate {
                count: Some(self.count),
                titles: Vec::new(),
            }),
            ..Layout::default()
        }
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.asked.lock().unwrap().push(req.page);
        let page = req.page.unwrap_or(0).min(self.count - 1);
        if self.fail == Some(page) {
            return Err(Failure::corrupt(t!(en: "page broken on purpose")));
        }
        if self.gif && page == 0 {
            let bytes = konoma_test_support::build_animated_gif(
                64,
                64,
                &[([255, 0, 0], 20), ([0, 255, 0], 20)],
            );
            let p = self.dir.join("anim.gif");
            std::fs::write(&p, bytes).expect("a gif is written");
            return Ok(file_view(&p));
        }
        if self.svg {
            let data = [TWO_TONE_SVG, BLUE_SVG, GREEN_SVG][page % 3];
            return Ok(View::Image(ImageView::new(ImageData::Svg {
                data: Arc::from(data.as_bytes()),
                base_dir: None,
                trusted: false,
            })));
        }
        let rgb = [[255u8, 0, 0], [0, 0, 255], [0, 255, 0]][page % 3];
        let img =
            image::RgbaImage::from_pixel(160, 160, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        Ok(file_view(&png_file(
            &self.dir,
            &format!("page{page}.png"),
            &img,
        )))
    }
}

use konoma_plugin::session::FailureKind;
use konoma_plugin::view::{Layout, PageLayout};
use konoma_plugin::Unavailable;

/// The pages asked for, one after the other with repeats of the same page folded.
fn pages_asked(asked: &Arc<Mutex<Vec<Option<usize>>>>) -> Vec<usize> {
    let mut v: Vec<usize> = asked
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.unwrap_or(0))
        .collect();
    v.dedup();
    v
}

fn run_paged(
    count: usize,
    svg: bool,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: Vec<Step>,
) -> (Out, Arc<Mutex<Vec<Option<usize>>>>) {
    run_paged_failing(count, svg, None, pictures, steps)
}

fn run_paged_failing(
    count: usize,
    svg: bool,
    fail: Option<usize>,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: Vec<Step>,
) -> (Out, Arc<Mutex<Vec<Option<usize>>>>) {
    run_paged_on_screen(count, svg, false, fail, pictures, |_| steps)
}

/// [`run_paged`] where the steps are made with the screen in hand (a step may resize it).
fn run_paged_on_screen(
    count: usize,
    svg: bool,
    gif: bool,
    fail: Option<usize>,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: impl FnOnce(&Arc<Mutex<TestBackend>>) -> Vec<Step>,
) -> (Out, Arc<Mutex<Vec<Option<usize>>>>) {
    let dir = unique_tmp("app_still_pages");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let path = dir.as_path().join("a.pic");
    std::fs::write(&path, "x").unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let plugin = PagedPlugin {
        count,
        asked: Arc::clone(&asked),
        dir: dir.as_path().to_path_buf(),
        svg,
        gif,
        fail,
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let waits = Arc::new(Mutex::new(Vec::new()));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: steps(&screen).into(),
        started: Instant::now(),
        log: Vec::new(),
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
    drop(dir);
    (
        Out {
            screen,
            calls: Arc::new(AtomicUsize::new(0)),
            waits,
        },
        asked,
    )
}

#[test]
fn j_and_k_ask_the_session_for_the_next_and_the_previous_page() {
    let (out, asked) = run_paged(
        3,
        false,
        half,
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(|b| has_blue(b) && footer(b).contains("2/3"), Some(key('J'))),
            step(
                |b| has_green(b) && footer(b).contains("3/3"),
                Some(key('K')),
            ),
            step(|b| has_blue(b) && footer(b).contains("2/3"), Some(key('K'))),
            step(|b| has_red(b) && footer(b).contains("1/3"), None),
        ],
    );
    assert_eq!(pages_asked(&asked), vec![0, 1, 2, 1, 0]);
    assert!(footer(out.screen.lock().unwrap().buffer()).contains("J/K/PgDn/PgUp:page"));
}

#[test]
fn page_down_and_page_up_turn_the_page_too() {
    let (_, asked) = run_paged(
        3,
        false,
        half,
        vec![
            step(
                |b| has_red(b) && footer(b).contains("1/3"),
                Some(code(KeyCode::PageDown)),
            ),
            step(
                |b| has_blue(b) && footer(b).contains("2/3"),
                Some(code(KeyCode::PageUp)),
            ),
            step(|b| has_red(b) && footer(b).contains("1/3"), None),
        ],
    );
    assert_eq!(pages_asked(&asked), vec![0, 1, 0]);
}

#[test]
fn the_page_stops_at_both_ends_and_does_not_wrap() {
    // At the first page `K` does nothing: the next `J` must reach page 2 (a wrap would have
    // moved to the last page and `J` on to the first).
    // At the last page `J` does nothing: the next `K` must reach page 2.
    let (_, asked) = run_paged(
        3,
        false,
        half,
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('K'))),
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('K'))),
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(|b| has_blue(b) && footer(b).contains("2/3"), Some(key('J'))),
            step(
                |b| has_green(b) && footer(b).contains("3/3"),
                Some(key('J')),
            ),
            step(
                |b| has_green(b) && footer(b).contains("3/3"),
                Some(key('J')),
            ),
            step(
                |b| has_green(b) && footer(b).contains("3/3"),
                Some(key('K')),
            ),
            step(|b| has_blue(b) && footer(b).contains("2/3"), None),
        ],
    );
    // The no-op keys asked for nothing.
    assert_eq!(pages_asked(&asked), vec![0, 1, 2, 1]);
}

#[test]
fn a_single_page_has_no_page_keys_and_says_1_of_1() {
    let (out, asked) = run_paged(
        1,
        false,
        half,
        vec![
            step(|b| has_red(b) && footer(b).contains("1/1"), Some(key('J'))),
            step(has_red, Some(key('K'))),
            step(has_red, Some(key('+'))),
            step(|b| has_red(b) && footer(b).contains("x1.25"), None),
        ],
    );
    assert_eq!(pages_asked(&asked), vec![0], "no page was asked for");
    let f = footer(out.screen.lock().unwrap().buffer());
    assert!(!f.contains("page"), "no page hint: {f}");
    assert!(f.contains("1/1"), "{f}");
}

#[test]
fn a_picture_without_pages_shows_no_number_and_no_page_hint() {
    let out = run_img(png_script(red_image), half, |_| {
        vec![step(|b| has_red(b) && footer(b).contains("x1.00"), None)]
    });
    let f = out.rows().pop().unwrap();
    let numbered = f.split_whitespace().any(|w| {
        w.split_once('/')
            .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
    });
    assert!(!f.contains("page") && !numbered, "{f}");
}

/// The cells the first page paints at fit (the size every page of these tests has at x1.00).
fn fit_cells(svg: bool) -> usize {
    let (out, _) = run_paged(3, svg, half, vec![step(has_red, None)]);
    let n = cells(out.screen.lock().unwrap().buffer(), is_picture);
    assert!(n >= 100, "the first page is drawn: {n}");
    n
}

#[test]
fn the_old_picture_stays_until_the_new_page_has_been_drawn() {
    let release = Arc::new(AtomicBool::new(false));
    let r_step = Arc::clone(&release);
    let mut turned = step(
        // The page number moved, the decode is out: the red page is still there, no loading line,
        // and the footer has the spinner.
        |b| {
            let f = footer(b);
            f.contains("2/3")
                && has_red(b)
                && !has_blue(b)
                && !shows(b, "loading")
                && f.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
        },
        None,
    );
    turned.act = Some(Box::new(move || r_step.store(true, Ordering::SeqCst)));
    turned.then = Some(AppEvent::Resize); // ignored by the loop; the next step takes over
                                          // Then the new page lands and stays: no answer meant for the old page may paint it over the
                                          // new one.
    let mut settled = 0;
    let stays = step(
        move |b| {
            if has_blue(b) && !has_red(b) && footer(b).contains("x1.00") {
                settled += 1;
            } else {
                settled = 0;
            }
            settled >= 300
        },
        None,
    );
    let (out, _) = run_paged(
        3,
        true,
        blue_held(&release),
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            turned,
            stays,
        ],
    );
    assert!(
        release.load(Ordering::SeqCst),
        "the decode was held and released"
    );
    let f = footer(out.screen.lock().unwrap().buffer());
    assert!(f.contains("2/3") && f.contains("x1.00"), "{f}");
}

#[test]
fn each_page_is_drawn_whole_and_alone_when_it_has_landed() {
    // The whole page (as many cells as the first at fit) and not a trace of the page before.
    let n = fit_cells(true);
    let (_, _) = run_paged(
        3,
        true,
        half,
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(
                move |b| {
                    cells(b, is_blue) == n
                        && !has_red(b)
                        && !has_green(b)
                        && footer(b).contains("2/3")
                },
                Some(key('J')),
            ),
            step(
                move |b| {
                    cells(b, is_green) == n
                        && !has_red(b)
                        && !has_blue(b)
                        && footer(b).contains("3/3")
                },
                None,
            ),
        ],
    );
}

#[test]
fn a_page_turn_returns_to_fit() {
    let n = fit_cells(false);
    let (out, _) = run_paged(
        3,
        false,
        half,
        vec![
            step(has_red, Some(key('+'))),
            step(|b| footer(b).contains("x1.25"), Some(key('+'))),
            step(|b| footer(b).contains("x1.56"), Some(code(KeyCode::Right))),
            step(|b| footer(b).contains("x1.56"), Some(key('J'))),
            // The new page is at x1.00 and drawn at the size of a fit, whole and centred.
            step(
                move |b| {
                    has_blue(b)
                        && footer(b).contains("2/3")
                        && footer(b).contains("x1.00")
                        && cells(b, is_picture) == n
                },
                None,
            ),
        ],
    );
    let b = out.screen.lock().unwrap();
    assert!(!footer(b.buffer()).contains("x1.56"));
}

/// The red and the yellow of the two-tone page: the yellow is more when the window has moved
/// down, as many as the red when it is centred.
fn tones(b: &Buffer) -> (usize, usize) {
    (cells(b, is_red), cells(b, is_yellow))
}

fn centred(b: &Buffer) -> bool {
    let (r, y) = tones(b);
    r >= 20 && r.abs_diff(y) <= 28
}

fn moved_down(b: &Buffer) -> bool {
    let (r, y) = tones(b);
    y >= r + 32
}

#[test]
fn zoom_pan_and_reset_act_on_the_old_page_while_the_new_one_is_awaited() {
    let release = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&release);
    let n = fit_cells(true);
    let mut settled = 0;
    let (out, _) = run_paged(
        3,
        true,
        blue_held(&release),
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            // Waiting for page 2: the old page is at fit.
            step(
                |b| centred(b) && footer(b).contains("2/3") && footer(b).contains("x1.00"),
                Some(key('+')),
            ),
            // The footer's zoom is the zoom of what is drawn: the window is zoomed and centred...
            step(
                |b| centred(b) && footer(b).contains("x1.25") && footer(b).contains("2/3"),
                Some(key('j')),
            ),
            // ...and a pan moves it (the footer still says the same zoom)...
            step(
                move |b| moved_down(b) && footer(b).contains("x1.25") && cells(b, is_picture) >= n,
                Some(key('=')),
            ),
            // ...and the reset puts it back at fit.
            step(
                |b| centred(b) && footer(b).contains("x1.00") && footer(b).contains("2/3"),
                Some(key('+')),
            ),
            {
                let mut s = step(
                    |b| centred(b) && footer(b).contains("x1.25") && !has_blue(b),
                    Some(AppEvent::Resize),
                );
                s.act = Some(Box::new(move || r.store(true, Ordering::SeqCst)));
                s
            },
            step(
                move |b| {
                    // What was zoomed while waiting stays on the new page, as on the host.
                    if has_blue(b) && !has_red(b) && footer(b).contains("x1.25") {
                        settled += 1;
                    } else {
                        settled = 0;
                    }
                    settled >= 200
                },
                None,
            ),
        ],
    );
    assert!(release.load(Ordering::SeqCst));
    assert!(has_blue(out.screen.lock().unwrap().buffer()));
}

#[test]
fn j_on_a_zoomed_page_returns_the_old_picture_to_fit_at_once() {
    let release = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&release);
    let (out, _) = run_paged(
        3,
        true,
        blue_held(&release),
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('+'))),
            step(|b| footer(b).contains("x1.25"), Some(key('+'))),
            step(|b| footer(b).contains("x1.56"), Some(key('j'))),
            step(
                |b| moved_down(b) && footer(b).contains("x1.56"),
                Some(key('J')),
            ),
            // The new page is still being decoded (the blue is held), and the old one is already
            // back at fit and centred, the footer saying so.
            {
                let mut s = step(
                    |b| {
                        centred(b)
                            && !has_blue(b)
                            && footer(b).contains("x1.00")
                            && footer(b).contains("2/3")
                    },
                    Some(AppEvent::Resize),
                );
                s.act = Some(Box::new(move || r.store(true, Ordering::SeqCst)));
                s
            },
            step(
                |b| has_blue(b) && !has_red(b) && footer(b).contains("x1.00"),
                None,
            ),
        ],
    );
    assert!(release.load(Ordering::SeqCst));
    assert!(has_blue(out.screen.lock().unwrap().buffer()));
}

#[test]
fn j_pressed_again_while_waiting_lands_on_the_last_page_asked() {
    let release = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&release);
    // Page 2 (blue) is held; the second `J` asks for page 3 (green) before it lands.
    let third = step(|b| has_red(b) && footer(b).contains("2/3"), Some(key('J')));
    let mut settled = 0;
    let (out, asked) = run_paged(
        3,
        true,
        blue_held(&release),
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            third,
            step(
                move |b| has_green(b) && !has_red(b) && footer(b).contains("3/3"),
                Some(AppEvent::Resize),
            ),
            {
                // The page that was cancelled never paints, even when its decoder is let go.
                let mut s = step(
                    move |b| {
                        if has_green(b) && !has_blue(b) && !has_red(b) && footer(b).contains("3/3")
                        {
                            settled += 1;
                        } else {
                            settled = 0;
                        }
                        settled >= 200
                    },
                    None,
                );
                s.act = Some(Box::new(move || r.store(true, Ordering::SeqCst)));
                s
            },
        ],
    );
    assert_eq!(pages_asked(&asked), vec![0, 1, 2]);
    let b = out.screen.lock().unwrap();
    assert!(has_green(b.buffer()) && !has_blue(b.buffer()));
}

#[test]
fn a_session_that_fails_while_a_page_is_awaited_shows_why_and_the_keys_turn_on() {
    let release = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&release);
    // Page 2 (blue) is held; page 3 fails in the session.
    let mut back = step(
        |b| shows(b, "page broken on purpose") && footer(b).contains("3/3") && !has_red(b),
        Some(key('K')),
    );
    back.act = Some(Box::new(move || r.store(true, Ordering::SeqCst)));
    let (out, asked) = run_paged_failing(
        3,
        true,
        Some(2),
        blue_held(&release),
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(|b| has_red(b) && footer(b).contains("2/3"), Some(key('J'))),
            back,
            // The page that failed did not roll the number back, `K` goes to page 2, which lands.
            step(
                |b| has_blue(b) && !shows(b, "broken") && footer(b).contains("2/3"),
                None,
            ),
        ],
    );
    assert_eq!(pages_asked(&asked), vec![0, 1, 2, 1]);
    assert!(release.load(Ordering::SeqCst));
    let b = out.screen.lock().unwrap();
    assert!(has_blue(b.buffer()));
}

#[test]
fn a_terminal_resize_while_a_page_is_awaited_and_after_it_lands_leaves_only_the_new_page() {
    // The halfblocks path asks a worker to resize the picture for the cells it is drawn in; an
    // answer meant for the page before must never be drawn over the page that replaced it.
    let release = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&release);
    let (out, _) = run_paged_on_screen(3, true, false, None, blue_held(&release), move |screen| {
        let resize_to = |w: u16, h: u16| {
            let screen = Arc::clone(screen);
            Box::new(move || screen.lock().unwrap().resize(w, h)) as Box<dyn FnOnce()>
        };
        let mut during = step(
            |b| has_red(b) && footer(b).contains("2/3"),
            Some(AppEvent::Resize),
        );
        during.act = Some(resize_to(W - 20, H + 4));
        let mut release_step = step(
            |b| has_red(b) && footer(b).contains("2/3") && b.area.width == W - 20,
            Some(AppEvent::Resize),
        );
        release_step.act = Some(Box::new(move || r.store(true, Ordering::SeqCst)));
        let mut after = step(
            |b| has_blue(b) && !has_red(b) && footer(b).contains("2/3"),
            Some(AppEvent::Resize),
        );
        after.act = Some(resize_to(W, H));
        let mut settled = 0;
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            during,
            release_step,
            after,
            step(
                move |b| {
                    // Settled at the first size on the new page, for many turns of the loop.
                    if has_blue(b) && !has_red(b) && b.area.width == W {
                        settled += 1;
                    } else {
                        settled = 0;
                    }
                    settled >= 300
                },
                None,
            ),
        ]
    });
    assert!(release.load(Ordering::SeqCst));
    assert!(has_blue(out.screen.lock().unwrap().buffer()));
}

#[test]
fn the_footer_and_the_help_name_the_page_keys_only_on_a_picture_with_pages() {
    let help_rows = |count| {
        let (out, _) = run_paged(
            count,
            false,
            half,
            vec![
                step(has_red, Some(key('?'))),
                step(|b| shows(b, "Preview: image"), None),
            ],
        );
        out.rows()
    };
    let paged = help_rows(3);
    let single = help_rows(1);
    assert!(paged.iter().any(|r| r.contains("J / K")), "{paged:#?}");
    assert!(!single.iter().any(|r| r.contains("J / K")), "{single:#?}");
}

#[test]
fn the_page_keys_act_and_are_named_exactly_when_the_picture_has_pages() {
    use konoma_plugin::app::{ImageMode, StillShape};
    let k = AppKeys::defaults();
    let (mut asked, mut acted) = (0, 0);
    for shape in [StillShape::Loading, StillShape::Ready, StillShape::Failed] {
        for paged in [false, true] {
            for zoomed in [false, true] {
                let mode = ImageMode {
                    shape,
                    paged,
                    zoomed,
                };
                for helping in [false, true] {
                    for c in ['J', 'K'] {
                        let acts = k.image_acts_on(&Key::char(c), helping, mode);
                        assert_eq!(acts, paged && !helping, "{c} {mode:?} help={helping}");
                        asked += 1;
                        acted += usize::from(acts);
                    }
                    for code in [KeyCode::PageDown, KeyCode::PageUp] {
                        let acts = k.image_acts_on(&Key::new(code.clone()), helping, mode);
                        assert_eq!(acts, paged && !helping, "{code:?} {mode:?}");
                    }
                }
                let named = |v: &[String]| v.iter().any(|t| t.contains("page"));
                assert_eq!(named(&k.image_footer_tokens(paged)), paged);
            }
        }
    }
    assert_eq!(asked, 3 * 2 * 2 * 2 * 2);
    assert_eq!(acted, 3 * 2 * 2);
}

// ---- an animation, through the real loop ----

/// A two-frame animation of 20 ms frames, red then green, as a view.
fn gif_script(_: usize, dir: &Path) -> View {
    let bytes =
        konoma_test_support::build_animated_gif(64, 64, &[([255, 0, 0], 20), ([0, 255, 0], 20)]);
    let p = dir.join("anim.gif");
    std::fs::write(&p, bytes).expect("a gif is written");
    file_view(&p)
}

/// The waits the loop asked for while `on` was on screen.
fn waits_on(out: &Out, on: &[char]) -> Vec<Option<Duration>> {
    out.waits
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, c)| on.contains(c))
        .map(|(w, _)| *w)
        .collect()
}

#[test]
fn an_animation_plays_through_the_loop_on_its_own_wakeups_and_every_wait_is_bounded() {
    let seen = Arc::new(Mutex::new(String::new()));
    let s = Arc::clone(&seen);
    // Red, then green, then red again: the frames come and the loop wraps, with no key.
    let out = run_img(gif_script, half, move |_| {
        vec![step(
            move |b| {
                let mut s = s.lock().unwrap();
                let now = if has_red(b) {
                    'r'
                } else if has_green(b) {
                    'g'
                } else {
                    return false;
                };
                if !s.ends_with(now) {
                    s.push(now);
                }
                s.as_str() == "rgr"
            },
            None,
        )]
    });
    assert_eq!(*seen.lock().unwrap(), "rgr");
    let w = waits_on(&out, &['r', 'g']);
    assert!(w.len() >= 3, "the loop was woken for the frames: {w:?}");
    for w in w {
        let w = w.expect("a playing animation never lets the loop sleep without a deadline");
        assert!(
            w >= Duration::from_millis(10) && w <= Duration::from_millis(100),
            "wait {w:?} is out of the 10..=100 ms bounds"
        );
    }
    assert_eq!(out.calls.load(Ordering::SeqCst), 1, "playing asks no one");
}

#[test]
fn a_still_picture_lets_the_loop_sleep_with_no_deadline() {
    let out = run_img(png_script(red_image), half, |_| vec![step(has_red, None)]);
    let w = waits_on(&out, &['r']);
    assert!(
        !w.is_empty(),
        "the loop asked for a wait with the picture on screen"
    );
    assert!(
        w.iter().all(Option::is_none),
        "a picture with no animation woke the loop: {w:?}"
    );
}

#[test]
fn turning_from_an_animation_to_a_still_page_stops_it_and_leaves_no_old_frame() {
    let seen = Arc::new(Mutex::new(String::new()));
    let s = Arc::clone(&seen);
    let s_turn = Arc::clone(&seen);
    // The page with the blue picture is page 1 (pages are red, blue, green).
    let (out, _) = run_paged_on_screen(2, false, true, None, half, move |_| {
        vec![
            step(has_red, Some(key('J'))),
            step(
                move |b| {
                    // From the turn on, a red or a green cell is a frame of the animation that
                    // should be gone.
                    if has_blue(b) {
                        return true;
                    }
                    if has_red(b) || has_green(b) {
                        s.lock().unwrap().push('x');
                    }
                    false
                },
                Some(key('+')),
            ),
            // Give a stale animation time to show up: many frames of 20 ms pass.
            {
                let turns = Arc::new(AtomicUsize::new(0));
                let s = s_turn;
                step(
                    move |b| {
                        if has_red(b) || has_green(b) {
                            s.lock().unwrap().push('y');
                        }
                        turns.fetch_add(1, Ordering::SeqCst) >= 150
                    },
                    None,
                )
            },
        ]
    });
    let stale = seen.lock().unwrap().clone();
    assert!(
        !stale.contains('y'),
        "a frame of the animation came back after the turn: {stale:?}"
    );
    // On the blue page nothing is playing: the loop sleeps with no deadline.
    let w = waits_on(&out, &['b']);
    assert!(!w.is_empty());
    assert!(
        w.last().unwrap().is_none(),
        "the animation kept waking the loop on a still page: {w:?}"
    );
}

#[test]
fn turning_from_an_animation_to_a_failing_page_stops_it_and_leaves_no_frame() {
    let stale = Arc::new(AtomicBool::new(false));
    let turns = Arc::new(AtomicUsize::new(0));
    let (st, tu) = (Arc::clone(&stale), Arc::clone(&turns));
    let (out, asked) = run_paged_failing_gif(2, Some(1), half, move |_| {
        vec![
            step(has_red, Some(key('J'))),
            step(
                |b| rows(b).iter().any(|l| l.contains("page broken on purpose")),
                Some(key('+')),
            ),
            // Many frames of 20 ms pass: a stale animation would show up as color.
            step(
                move |b| {
                    if has_red(b) || has_green(b) {
                        st.store(true, Ordering::SeqCst);
                    }
                    tu.fetch_add(1, Ordering::SeqCst) >= 150
                },
                None,
            ),
        ]
    });
    assert_eq!(pages_asked(&asked), [0, 1]);
    assert!(
        !stale.load(Ordering::SeqCst),
        "a frame of the animation stayed on screen after the page failed"
    );
    let w = waits_on(&out, &['-']);
    assert!(
        w.last().is_some_and(Option::is_none),
        "the animation kept waking the loop on a failed page: {w:?}"
    );
}

fn run_paged_failing_gif(
    count: usize,
    fail: Option<usize>,
    pictures: impl FnOnce(&Path) -> Option<Pictures>,
    steps: impl FnOnce(&Arc<Mutex<TestBackend>>) -> Vec<Step>,
) -> (Out, Arc<Mutex<Vec<Option<usize>>>>) {
    run_paged_on_screen(count, false, true, fail, pictures, steps)
}

// ---- a page that fails after a page that drew: what the failure changes ----

/// Page 0 is a red picture; page 1 fails with `kind` (and, when it is asked for, the second
/// page of a file that is also on disk as `a.pic`, for the raw-text words).
struct FlakyPlugin {
    dir: PathBuf,
    kind: FailureKind,
    calls: Arc<AtomicUsize>,
}

struct FlakySession {
    dir: PathBuf,
    kind: FailureKind,
    calls: Arc<AtomicUsize>,
}

impl Plugin for FlakyPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("img-test", "0.0.1")
            .bind(Bind::files(["*.pic"]))
            .shows(ViewKind::Image)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(FlakySession {
            dir: self.dir.clone(),
            kind: self.kind,
            calls: Arc::clone(&self.calls),
        }))
    }
}

impl Session for FlakySession {
    fn layout(&self) -> Layout {
        Layout {
            pages: Some(PageLayout::Separate {
                count: Some(3),
                titles: Vec::new(),
            }),
            ..Layout::default()
        }
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if req.page.unwrap_or(0) == 1 {
            return Err(Failure::new(self.kind, t!(en: "page broken on purpose")));
        }
        Ok(file_view(&png_file(&self.dir, "flaky.png", &red_image())))
    }
}

/// Run the app over [`FlakyPlugin`] (`body` is the file's text) with the steps made from the
/// number of `view` calls so far.
fn run_flaky(
    kind: FailureKind,
    unavailable: Unavailable,
    body: &str,
    steps: impl FnOnce(&Arc<AtomicUsize>) -> Vec<Step>,
) -> (Out, Arc<AtomicUsize>) {
    run_flaky_shown(
        konoma_plugin::app::Display::default(),
        kind,
        unavailable,
        body,
        steps,
    )
}

/// [`run_flaky`] with the `[ui]` display settings `display`.
fn run_flaky_shown(
    display: konoma_plugin::app::Display,
    kind: FailureKind,
    unavailable: Unavailable,
    body: &str,
    steps: impl FnOnce(&Arc<AtomicUsize>) -> Vec<Step>,
) -> (Out, Arc<AtomicUsize>) {
    let dir = unique_tmp("app_still_flaky");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let path = dir.as_path().join("a.pic");
    std::fs::write(&path, body).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = FlakyPlugin {
        dir: dir.as_path().to_path_buf(),
        kind,
        calls: Arc::clone(&calls),
    };
    let mut opened =
        open_file(vec![Arc::new(plugin)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let waits = Arc::new(Mutex::new(Vec::new()));
    let mut events = Driver {
        screen: Arc::clone(&screen),
        steps: steps(&calls).into(),
        started: Instant::now(),
        log: Vec::new(),
        waits: Arc::clone(&waits),
    };
    let pictures = half(&opened.path);
    let cx = AppCx {
        unavailable,
        plugin_id: "img-test".into(),
        path: opened.path.clone(),
        display: display.clone(),
        ..cx()
    };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults().with_wrap(display.wrap),
        pictures,
    )
    .expect("runs");
    drop(dir);
    (
        Out {
            screen,
            calls: Arc::new(AtomicUsize::new(0)),
            waits,
        },
        calls,
    )
}

#[test]
fn a_cancelled_page_changes_nothing_on_screen_whatever_the_plugin_declared() {
    for unavailable in [
        Unavailable::Picture,
        Unavailable::RawText { with_reason: true },
        Unavailable::Notice(t!(en: "[x] nothing to show")),
        Unavailable::Message,
    ] {
        let (out, calls) = run_flaky(FailureKind::Cancelled, unavailable.clone(), "x", |calls| {
            let c = Arc::clone(calls);
            vec![
                step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
                // Page 1 was asked for (and said "cancelled"): one more event to draw after it.
                step(move |_| c.load(Ordering::SeqCst) >= 2, Some(key('x'))),
                step(|_| true, None),
            ]
        });
        assert!(calls.load(Ordering::SeqCst) >= 2, "{unavailable:?}");
        let r = out.rows();
        let b = out.screen.lock().unwrap();
        assert!(
            has_red(b.buffer()),
            "{unavailable:?}: the picture stays\n{r:#?}"
        );
        for words in [
            "cannot display",
            "broken on purpose",
            "nothing to show",
            "not drawn",
        ] {
            assert!(
                !r.iter().any(|l| l.contains(words)),
                "{unavailable:?}: cancelled says {words:?}\n{r:#?}"
            );
        }
    }
}

#[test]
fn a_page_that_fails_with_its_file_as_text_scrolls_and_keeps_the_page_keys() {
    let body: String = (0..90).map(|i| format!("junk-line-{i:03}\n")).collect();
    let (out, _) = run_flaky(
        FailureKind::Corrupt,
        Unavailable::RawText { with_reason: true },
        &body,
        |_| {
            vec![
                step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
                step(|b| shows(b, "junk-line-000"), Some(key('G'))),
                step(
                    |b| shows(b, "junk-line-089") && !shows(b, "junk-line-000"),
                    Some(key('g')),
                ),
                // The reason and a blank row are above the body: three lines scroll 000 away.
                step(|b| shows(b, "junk-line-000"), Some(key('j'))),
                step(|_| true, Some(key('j'))),
                step(|_| true, Some(key('j'))),
                step(|b| !shows(b, "junk-line-000"), Some(key('K'))),
                // The page keys turned on: `K` goes back to page 1, which draws.
                step(has_red, None),
            ]
        },
    );
    assert!(has_red(out.screen.lock().unwrap().buffer()));
}

/// The file's text of a page that failed to draw wraps like any text (`[ui] wrap`), or scrolls
/// sideways when it does not.
#[test]
fn a_failed_pages_text_wraps_or_scrolls_sideways_by_ui_wrap() {
    // One line of 130 cells in a 100-wide terminal: 100 of `a`..., then a tail.
    let body = format!("{}TAILMARK\n", "a".repeat(130));
    let raw = || Unavailable::RawText { with_reason: true };
    // Wrapping (the default): the tail is on the next row.
    let (out, _) = run_flaky(FailureKind::Corrupt, raw(), &body, |_| {
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(|b| shows(b, "TAILMARK"), None),
        ]
    });
    assert!(shows(out.screen.lock().unwrap().buffer(), "TAILMARK"));
    // Not wrapping: the tail is off screen until `$`, and `0` brings the start back.
    let display = konoma_plugin::app::Display {
        wrap: false,
        ..Default::default()
    };
    let (out, _) = run_flaky_shown(display, FailureKind::Corrupt, raw(), &body, |_| {
        vec![
            step(|b| has_red(b) && footer(b).contains("1/3"), Some(key('J'))),
            step(
                |b| shows(b, "aaaaaaaa") && !shows(b, "TAILMARK"),
                Some(key('$')),
            ),
            step(|b| shows(b, "TAILMARK"), Some(key('0'))),
            step(|b| !shows(b, "TAILMARK"), None),
        ]
    });
    assert!(!shows(out.screen.lock().unwrap().buffer(), "TAILMARK"));
}
