//! An animated GIF in the konoma host and in the standalone app (`konoma-plugin` feature `app`):
//! both play it with the shared `GifPlayer`, so on the same frame, with the same keys, they must
//! show the same cells, the same zoom, the same footer hints and the same help rows as for a
//! still picture (the host adds nothing of its own for a GIF).
//!
//! Nothing waits on a clock. The host is put on a frame with a test seam. The app plays by the
//! real clock, so its GIFs are made to need no waiting: the frame the app must stop on has a delay
//! of ten minutes, and the one before it (when there is one) lasts 20 ms; the event source gives
//! the keys only once the screen shows that frame, and a screen that never gets there fails the
//! test at a deadline.

use super::image_parity::{
    app_zoom, host_shot_at, host_zoom, image_section, PngPlugin, Shot, HOST_ONLY, HOST_ONLY_ROWS,
};
use super::table_parity::{help_lines, hints, presses, same_row, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, open_file, run_with_pictures, AppCx, AppEvent, AppKeys, EventSource, Next,
    Pictures,
};
use konoma_plugin::TermCaps;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::Color as RColor;
use ratatui_image::picker::Picker;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const LONG: u32 = 600_000;

/// A `TestBackend` the event source can look at while the loop draws on it.
pub(super) struct Shared(pub(super) Arc<Mutex<TestBackend>>);

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

fn near(col: RColor, want: [u8; 3]) -> bool {
    matches!(col, RColor::Rgb(r, g, b)
        if r.abs_diff(want[0]) < 40 && g.abs_diff(want[1]) < 40 && b.abs_diff(want[2]) < 40)
}

fn is_color(c: &Cell, want: [u8; 3]) -> bool {
    near(c.fg, want) || near(c.bg, want)
}

fn shows_color(b: &Buffer, want: [u8; 3]) -> bool {
    b.content().iter().filter(|c| is_color(c, want)).count() >= 8
}

/// What the screen has to show before the keys are given.
#[derive(Clone, Copy)]
enum Until {
    /// A frame, told by its color.
    Color([u8; 3]),
    /// The line that says a picture cannot be shown.
    Failure,
    /// A row with this text.
    Text(&'static str),
    /// Nothing (there is no picture backend).
    Nothing,
}

impl Until {
    fn met(self, b: &Buffer) -> bool {
        match self {
            Until::Color(c) => shows_color(b, c),
            Until::Failure => (0..b.area.height).any(|y| {
                let row: String = (0..b.area.width).map(|x| b[(x, y)].symbol()).collect();
                row.contains("[image]") || row.contains("cannot display image")
            }),
            Until::Text(t) => (0..b.area.height).any(|y| {
                let row: String = (0..b.area.width).map(|x| b[(x, y)].symbol()).collect();
                row.contains(t)
            }),
            Until::Nothing => true,
        }
    }
}

/// The keys one after the other once the screen shows `want`, then the end.
struct OnceShowing {
    screen: Arc<Mutex<TestBackend>>,
    want: Until,
    seen: bool,
    keys: std::collections::VecDeque<AppEvent>,
    started: Instant,
}

impl EventSource for OnceShowing {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if !self.seen {
            self.seen = self.want.met(self.screen.lock().unwrap().buffer());
        }
        if !self.seen {
            assert!(
                self.started.elapsed() < Duration::from_secs(20),
                "the app never got to the state the keys wait for"
            );
            std::thread::sleep(wait.unwrap_or_default().min(Duration::from_millis(2)));
            return Next::Timeout;
        }
        match self.keys.pop_front() {
            Some(k) => Next::Event(k),
            None => Next::End,
        }
    }
}

fn shot_of(buf: &Buffer) -> Shot {
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            // A wide glyph's trailing cell is skipped, as the host's `rows_of` reads the buffer.
            let mut line = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect();
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

/// What the app shows for the GIF `path` after the presses, once it stands on the frame that
/// meets `until`. `pictures` false: no picture backend.
fn app_gif_shot(path: &std::path::Path, h: u16, spec: &str, until: Until, pictures: bool) -> Shot {
    let mut opened =
        open_file(vec![Arc::new(PngPlugin)], path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, h)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        ..AppCx::default()
    };
    let keys = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = OnceShowing {
        screen: Arc::clone(&screen),
        want: if pictures { until } else { Until::Nothing },
        seen: false,
        keys,
        started: Instant::now(),
    };
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
    assert!(events.keys.is_empty(), "the app ended early: {spec}");
    let guard = screen.lock().unwrap();
    shot_of(guard.buffer())
}

/// GIFs of two frames (red, then blue) in bands, so a pan changes the cells: `a` (square),
/// `wide` and `tall`. The last frame lasts [`LONG`] ms and the first 20 ms (the app moves on to
/// the second at once) or [`LONG`] ms (it stays on the first). A frame is told by its color.
fn fixtures() -> crate::test_support::TmpDir {
    let dir = sandbox("image_gif_parity");
    let write = |name: &str, first_ms: u32, w: u32, h: u32| {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for (c, ms) in [(RED, first_ms), (BLUE, LONG)] {
                // A frame with a stripe, so a pan changes the cells and not just the whole fill.
                let img = RgbaImage::from_fn(w, h, |x, y| {
                    let k = ((x / 40 + y / 40) % 2) as u8;
                    Rgba([
                        c[0].saturating_sub(k * 90),
                        c[1].saturating_sub(k * 90),
                        c[2].saturating_sub(k * 90),
                        255,
                    ])
                });
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(ms, 1),
                ))
                .unwrap();
            }
        }
        std::fs::write(dir.join(name), out).unwrap();
    };
    // `<name>.gif` goes on to its second frame at once; `<name>0.gif` stays on its first.
    for (name, w, h) in [("a", 320, 320), ("wide", 400, 100), ("tall", 100, 400)] {
        write(&format!("{name}.gif"), 20, w, h);
        write(&format!("{name}0.gif"), LONG, w, h);
    }
    // The same bytes as a one-frame GIF: a still picture in both.
    std::fs::write(
        dir.join("one.gif"),
        konoma_test_support::build_animated_gif(160, 160, &[(RED, 100)]),
    )
    .unwrap();
    std::fs::write(dir.join("junk.gif"), b"GIF89a this is not a gif").unwrap();
    dir
}

const SPECS: &[&str] = &[
    "",
    "+",
    "+ +",
    "+ + + +",
    "-",
    "+ -",
    "+ =",
    "=",
    "h j k l",
    "+ h",
    "+ l",
    "+ j",
    "+ k",
    "+ + k",
    "+ + + l",
    "+ + + + l l l",
    "+ + + + + j j j j l l l l",
];

#[test]
fn the_cells_the_zoom_and_the_footer_of_a_gif_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let (mut checked, mut moved) = (0, 0);
    let mut seen_host_only = std::collections::BTreeSet::new();
    let mut zooms = std::collections::BTreeSet::new();
    let mut frames_seen = std::collections::BTreeSet::new();
    for file in ["a.gif", "wide.gif", "tall.gif"] {
        for (idx, color) in [(0usize, RED), (1, BLUE)] {
            let file = &if idx == 0 {
                file.replace(".gif", "0.gif")
            } else {
                file.to_string()
            };
            let at_fit = host_shot_at(&root, file, HOST_H, "", true, Some(idx));
            for spec in SPECS {
                let what = format!("{file} frame {idx} [{spec}]");
                let h = host_shot_at(&root, file, HOST_H, spec, true, Some(idx));
                let a = app_gif_shot(&root.join(file), APP_H, spec, Until::Color(color), true);
                assert!(h.cells.len() > 400, "{what}: the host frame is there");
                assert_eq!(h.cells.len(), a.cells.len(), "{what}: frame sizes differ");
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
                let on = |cells: &[(String, RColor, RColor)], want: [u8; 3]| {
                    cells
                        .iter()
                        .filter(|c| near(c.1, want) || near(c.2, want))
                        .count()
                        >= 8
                };
                assert!(
                    on(&a.cells, color),
                    "{what}: the frame's color is on screen"
                );
                frames_seen.insert(idx);
                let (hz, az) = (host_zoom(&h.rows), app_zoom(&a.rows));
                assert!(
                    hz.is_some(),
                    "{what}: the host shows a zoom: {:?}",
                    h.rows[0]
                );
                assert_eq!(hz, az, "{what}: the zoom differs");
                zooms.insert(hz.unwrap());
                let (hh, ah) = (
                    hints(h.rows.last().expect("footer")),
                    hints(a.rows.last().expect("footer")),
                );
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
                        "{what}: the app footer has `{t}` the host lacks"
                    );
                }
                checked += 1;
                moved += usize::from(h.cells != at_fit.cells);
            }
        }
    }
    assert!(checked >= 3 * 2 * 17, "{checked}");
    assert_eq!(frames_seen.len(), 2);
    // The keys moved the picture, and took it through many zooms.
    assert!(moved >= 50, "{moved}");
    assert!(zooms.len() >= 5, "{zooms:?}");
    for p in HOST_ONLY {
        assert!(seen_host_only.contains(*p), "HOST_ONLY `{p}` never occurs");
    }
}

#[test]
fn a_gif_of_one_frame_and_a_broken_gif_are_shown_as_in_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    // One frame: a still picture (the app's halfblocks resize is answered on a worker, so the
    // comparison waits for the picture as the other parity tests do: with `WhenIdle`).
    let h = host_shot_at(&root, "one.gif", HOST_H, "+", true, None);
    let mut opened = open_file(
        vec![Arc::new(PngPlugin)],
        &root.join("one.gif"),
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(W, APP_H)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        ..AppCx::default()
    };
    let keys = presses("+")
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = super::image_parity::WhenIdle { keys, turns: 0 };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        Some(Pictures::new(
            Picker::halfblocks(),
            "png-parity",
            &opened.path,
            800,
        )),
    )
    .unwrap();
    let a = super::image_parity::shot(&term);
    assert_eq!(h.cells, a.cells, "a one-frame GIF is drawn as a still");
    assert_eq!(host_zoom(&h.rows), app_zoom(&a.rows));
    assert!(h.cells.len() > 400);
    // A broken GIF: the same words.
    let line = |rows: &[String]| {
        rows.iter()
            .find(|r| r.contains("[image]") || r.contains("cannot display image"))
            .map(|r| r.trim_matches(|c| c == '│' || c == ' ').to_string())
    };
    let h = host_shot_at(&root, "junk.gif", HOST_H, "", true, None);
    let a = app_gif_shot(&root.join("junk.gif"), APP_H, "", Until::Failure, true);
    assert!(line(&h.rows).is_some(), "{}", h.rows.join("\n"));
    assert_eq!(line(&h.rows), line(&a.rows));
}

#[test]
fn the_help_rows_of_a_gif_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let (mut compared, mut host_only) = (0, std::collections::BTreeSet::new());
    for file in ["a0.gif", "wide0.gif"] {
        for spec in ["?", "+ ?", "+ + + ?", "+ = ?", "h ?", "+ + + + + j l ?"] {
            let what = format!("{file} [{spec}]");
            let h = host_shot_at(&root, file, 70, spec, true, Some(0));
            let a = app_gif_shot(&root.join(file), 69, spec, Until::Color(RED), true);
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
        }
    }
    assert!(compared >= 2 * 6 * 3, "{compared}");
    assert_eq!(host_only.len(), HOST_ONLY_ROWS.len(), "{host_only:?}");
}

// ---- turning a page from an animation to a still or to a failure -------------------------------

use konoma_plugin::prelude::{
    Bind, Failure, Input, Manifest, OpenCx, Plugin, Session, View, ViewCx, ViewKind,
};
use konoma_plugin::t;
use konoma_plugin::view::{ImageData, ImageView, Layout, PageLayout, ViewReq};

/// A document of two pages: page 1 is the animation (or picture) file `gif`, page 2 the picture
/// file `second`, or a page that fails to draw when there is none; with `first_fails` page 1 is
/// the one that fails. Stands in for the plugin in the host (`STAND_IN`) and is the plugin of the
/// app, which declares the `unavailable` the host's table holds for the same kind of file.
struct TurnPlugin {
    gif: std::path::PathBuf,
    second: Option<std::path::PathBuf>,
    first_fails: bool,
    unavailable: konoma_plugin::Unavailable,
    /// The file name it is bound to.
    file: &'static str,
    /// The id of the host's plugin it stands in for.
    kind: &'static str,
    /// The color page 1 shows (the animation's first frame is red).
    before: [u8; 3],
    /// Keys pressed once the screen stands on the page it ends on (`"j j"`).
    then: &'static str,
    /// Whether the document has pages to turn (two), or none.
    paged: bool,
    /// A color the screen must show before the app ends (the page a last `J` turned to).
    end_on: Option<[u8; 3]>,
}

impl TurnPlugin {
    /// A picture plugin (the host's `image` one) bound to `turn.png`.
    fn new(gif: std::path::PathBuf, second: Option<std::path::PathBuf>) -> TurnPlugin {
        TurnPlugin {
            gif,
            second,
            first_fails: false,
            unavailable: crate::plugins::unavailable_of("image"),
            file: "turn.png",
            kind: "image",
            before: RED,
            then: "",
            paged: true,
            end_on: None,
        }
    }

    /// The same document as the plugin `id` of the host shows `file` (its declared
    /// `unavailable`, read from the host's table).
    fn of_kind(mut self, id: &'static str, file: &'static str) -> TurnPlugin {
        self.unavailable = crate::plugins::unavailable_of(id);
        self.file = file;
        self.kind = id;
        self
    }
}

struct TurnSession {
    gif: std::path::PathBuf,
    second: Option<std::path::PathBuf>,
    first_fails: bool,
    paged: bool,
}

impl Plugin for TurnPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("turn-parity", "0.0.1")
            .bind(Bind::files([self.file]))
            .shows(ViewKind::Image)
            .unavailable(self.unavailable.clone())
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(TurnSession {
            gif: self.gif.clone(),
            second: self.second.clone(),
            first_fails: self.first_fails,
            paged: self.paged,
        }))
    }
}

impl Session for TurnSession {
    fn layout(&self) -> Layout {
        Layout {
            pages: self.paged.then(|| PageLayout::Separate {
                count: Some(2),
                titles: Vec::new(),
            }),
            ..Layout::default()
        }
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let file = |p: &std::path::Path| Ok(View::Image(ImageView::new(ImageData::File(p.into()))));
        match (req.page.unwrap_or(0), &self.second) {
            (0, _) if self.first_fails => Err(Failure::corrupt(t!(en: "page broken on purpose"))),
            (0, _) => file(&self.gif),
            (_, Some(p)) => file(p),
            (_, None) => Err(Failure::corrupt(t!(en: "page broken on purpose"))),
        }
    }
}

/// The keys one after the other, each once the screen shows what it waits for (a frame's color,
/// or the failure line), then the end.
struct TurnSource {
    screen: Arc<Mutex<TestBackend>>,
    steps: std::collections::VecDeque<(Until, AppEvent)>,
    after: Until,
    started: Instant,
}

impl EventSource for TurnSource {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        let ready = {
            let g = self.screen.lock().unwrap();
            match self.steps.front() {
                Some((until, _)) => until.met(g.buffer()),
                None => self.after.met(g.buffer()),
            }
        };
        if !ready {
            assert!(
                self.started.elapsed() < Duration::from_secs(20),
                "the app never got to the state the next step waits for:\n{}",
                shot_of(self.screen.lock().unwrap().buffer())
                    .rows
                    .join("\n")
            );
            std::thread::sleep(wait.unwrap_or_default().min(Duration::from_millis(2)));
            return Next::Timeout;
        }
        match self.steps.pop_front() {
            Some((_, k)) => Next::Event(k),
            None => Next::End,
        }
    }
}

const GREEN: [u8; 3] = [0, 255, 0];

/// What the host shows after `J` on page 1 (the animation, standing on its first frame).
fn host_turn_shot(root: &std::path::Path, plugin: TurnPlugin) -> Shot {
    use crate::app::media_plugin::STAND_IN;
    let (file, first_fails, before, then) =
        (plugin.file, plugin.first_fails, plugin.before, plugin.then);
    let end_on = plugin.end_on;
    // The page the host stands on (of two, or of one without pages): a page key starts a job only
    // when it turns the page.
    let pages: usize = if plugin.paged { 2 } else { 1 };
    let mut at: usize = if first_fails { 0 } else { 1.min(pages - 1) };
    STAND_IN.with(|s| *s.borrow_mut() = Some(Arc::new(plugin)));
    let mut s = Sim::with_config_sized(root, cfg_en(), W, HOST_H)
        .with_picker()
        .with_media();
    let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel();
    s.app.attach_image_backend(Picker::halfblocks(), req_tx);
    s.select(file);
    s.enter();
    if first_fails {
        // The failure of the very first page may be handed over on the spot (no message left in
        // the channel to wait for), so `drain_media` cannot be used. The state says whether
        // anything is still to come: the preview's job has not landed while it is loading
        // (a layout sent ahead keeps the mark). Wait for messages only while that holds.
        let rx = s.media_rx.take().expect("with_media");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while s.app.is_media_loading() {
            let left = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("the first page's failure lands within 20 s");
            let res = rx
                .recv_timeout(left)
                .expect("the first page's failure lands within 20 s");
            s.app.apply_media(res);
        }
        s.media_rx = Some(rx);
        s.draw();
    } else {
        s.drain_media();
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
    if !first_fails {
        assert!(
            shows_color(s.term.backend().buffer(), before),
            "page 1 shows its picture\n{}",
            super::image_parity::shot(&s.term).rows.join("\n")
        );
        s.press(KeyCode::Char('J'), KeyModifiers::NONE);
        s.drain_media();
        settle(&mut s, &mut req_rx);
    }
    for (code, mods) in presses(then) {
        // Scrolling a failed page starts no job: there is nothing to wait for.
        s.press(code, mods);
        let turned = match code {
            KeyCode::Char('J') if at + 1 < pages => {
                at += 1;
                true
            }
            KeyCode::Char('K') if at > 0 => {
                at -= 1;
                true
            }
            _ => false,
        };
        if end_on.is_some() || turned {
            // A page turn starts one.
            s.drain_media();
        }
        s.draw();
        settle(&mut s, &mut req_rx);
    }
    STAND_IN.with(|s| *s.borrow_mut() = None);
    super::image_parity::shot(&s.term)
}

/// What the app shows after `J`, once it shows what the host ended on (`after`).
fn app_turn_shot(path: &std::path::Path, plugin: TurnPlugin, after: Until) -> Shot {
    let end_on = plugin.end_on;
    let (unavailable, kind, first_fails, before, then) = (
        plugin.unavailable.clone(),
        plugin.kind,
        plugin.first_fails,
        plugin.before,
        plugin.then,
    );
    let mut opened = open_file(vec![Arc::new(plugin)], path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, APP_H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable,
        plugin_id: kind.into(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
    };
    let key = presses("J")
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect::<Vec<_>>()
        .remove(0);
    let mut steps: std::collections::VecDeque<(Until, AppEvent)> = if first_fails {
        std::collections::VecDeque::new()
    } else {
        [(Until::Color(before), key)].into()
    };
    // The keys after the page it ends on: each waits for that page, and the end waits for
    // nothing (the text they scroll may be gone from the screen).
    for (i, (code, mods)) in presses(then).into_iter().enumerate() {
        let k = AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"));
        // Only the first waits for the page: once it scrolls, that page's text may be gone.
        steps.push_back((if i == 0 { after } else { Until::Nothing }, k));
    }
    let after = match end_on {
        Some(c) => Until::Color(c),
        None if then.is_empty() => after,
        None => Until::Nothing,
    };
    let mut events = TurnSource {
        screen: Arc::clone(&screen),
        steps,
        after,
        started: Instant::now(),
    };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        Some(Pictures::new(
            Picker::halfblocks(),
            "turn-parity",
            &opened.path,
            800,
        )),
    )
    .unwrap();
    assert!(events.steps.is_empty());
    let guard = screen.lock().unwrap();
    shot_of(guard.buffer())
}

fn failure_line(rows: &[String]) -> Option<String> {
    rows.iter()
        .find(|r| r.contains("[image]") || r.contains("cannot display image"))
        .map(|r| r.trim_matches(|c| c == '│' || c == ' ').to_string())
}

#[test]
fn turning_from_an_animation_to_a_still_page_or_a_failing_page_agrees_with_the_host() {
    let dir = fixtures();
    // The file the host opens (its bytes are not read: the stand-in plugin answers).
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    let root = canon(&dir);
    let plugin =
        |second: bool| TurnPlugin::new(root.join("a0.gif"), second.then(|| root.join("green.png")));
    // To a still picture: the same cells, and the animation's red is gone in both.
    let h = host_turn_shot(&root, plugin(true));
    let a = app_turn_shot(&root.join("turn.png"), plugin(true), Until::Color(GREEN));
    assert!(h.cells.len() > 400, "{}", h.rows.join("\n"));
    assert_eq!(h.cells.len(), a.cells.len());
    fn count(cells: &[(String, RColor, RColor)], want: [u8; 3]) -> usize {
        cells
            .iter()
            .filter(|c| near(c.1, want) || near(c.2, want))
            .count()
    }
    assert!(count(&h.cells, GREEN) >= 8 && count(&a.cells, GREEN) >= 8);
    assert_eq!(count(&h.cells, RED), 0, "host: the old animation is gone");
    assert_eq!(count(&a.cells, RED), 0, "app: the old animation is gone");
    assert_eq!(h.cells, a.cells, "the still page is drawn alike");
    // To a page whose picture is broken: the same words, and no animation left behind.
    let broken = |name: &str| TurnPlugin::new(root.join("a0.gif"), Some(root.join(name)));
    let h = host_turn_shot(&root, broken("junk.gif"));
    let a = app_turn_shot(&root.join("turn.png"), broken("junk.gif"), Until::Failure);
    assert!(failure_line(&h.rows).is_some(), "{}", h.rows.join("\n"));
    assert_eq!(failure_line(&h.rows), failure_line(&a.rows));
    assert_eq!(count(&h.cells, RED), 0, "host: the old animation is gone");
    assert_eq!(count(&a.cells, RED), 0, "app: the old animation is gone");
    // To a page the plugin fails to draw: the same words, neither keeps the red.
    let h = host_turn_shot(&root, plugin(false));
    let a = app_turn_shot(
        &root.join("turn.png"),
        plugin(false),
        Until::Text("[image]"),
    );
    assert_eq!(count(&h.cells, RED), 0, "host: the old animation is gone");
    assert_eq!(count(&a.cells, RED), 0, "app: the old animation is gone");
    assert!(failure_line(&h.rows).is_some(), "{}", h.rows.join("\n"));
    assert_eq!(failure_line(&h.rows), failure_line(&a.rows));
    assert_eq!(h.cells, a.cells, "the failing page is drawn alike");
}

/// The rows of what a screen says, without the frame's border and the status/footer rows: the
/// host draws its text in a bordered block under a status row, the app has no border.
fn content_rows(rows: &[String]) -> Vec<String> {
    let inner: Vec<&String> = match rows.iter().position(|r| r.starts_with('┌')) {
        Some(top) => {
            let end = rows.iter().position(|r| r.starts_with('└')).unwrap();
            rows[top + 1..end].iter().collect()
        }
        // The last row is the footer.
        None => rows[..rows.len() - 1].iter().collect(),
    };
    let mut out: Vec<String> = inner
        .iter()
        .map(|r| {
            r.trim_start_matches('│')
                .trim_end_matches(['│', '█', ' '])
                .to_string()
        })
        .collect();
    while out.last().is_some_and(|r| r.is_empty()) {
        out.pop();
    }
    out
}

/// One cell of the parity matrix: the plugin `id` (whose file is `file`) fails to draw either its
/// first page (`first_fails`) or the page after `before` (a still or the animation).
/// Returns the removal-proof numbers it checked.
fn failing_page_agrees(
    id: &'static str,
    file: &'static str,
    first_fails: bool,
    still_before: bool,
) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    std::fs::write(dir.join("turn.svg"), "<svg>junk-body</svg>").unwrap();
    std::fs::write(dir.join("turn.pdf"), "%PDF junk").unwrap();
    // The bytes of an mp4 file's first box: the host tells a video by its content.
    std::fs::write(
        dir.join("turn.mp4"),
        [&[0, 0, 0, 0x18][..], b"ftypmp42\0\0\0\0mp42isom"].concat(),
    )
    .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    let root = canon(&dir);
    let mk = || {
        let gif = if still_before { "green.png" } else { "a0.gif" };
        let mut p = TurnPlugin::new(root.join(gif), None).of_kind(id, file);
        p.first_fails = first_fails;
        p.before = if still_before { GREEN } else { RED };
        p
    };
    let what = format!("{id} first_fails={first_fails} still_before={still_before}");
    let until = match id {
        "image" => Until::Failure,
        "svg" => Until::Text("junk-body"),
        "video" => Until::Text("turn.mp4"),
        _ => Until::Text("turn.pdf"),
    };
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join(file), mk(), until);
    let old = if still_before { GREEN } else { RED };
    let count = |cells: &[(String, RColor, RColor)]| {
        cells
            .iter()
            .filter(|c| near(c.1, old) || near(c.2, old))
            .count()
    };
    assert_eq!(
        count(&h.cells),
        0,
        "{what}: host keeps the old picture\n{}",
        h.rows.join("\n")
    );
    assert_eq!(
        count(&a.cells),
        0,
        "{what}: app keeps the old picture\n{}",
        a.rows.join("\n")
    );
    if id == "image" {
        assert!(
            failure_line(&h.rows).is_some(),
            "{what}\n{}",
            h.rows.join("\n")
        );
        assert_eq!(failure_line(&h.rows), failure_line(&a.rows), "{what}");
        assert_eq!(h.cells, a.cells, "{what}: the failing page is drawn alike");
    } else {
        let (hr, ar) = (content_rows(&h.rows), content_rows(&a.rows));
        assert!(
            ar.len() >= 2,
            "{what}: the app says something\n{}",
            a.rows.join("\n")
        );
        assert_eq!(
            hr,
            ar,
            "{what}\nhost:\n{}\napp:\n{}",
            h.rows.join("\n"),
            a.rows.join("\n")
        );
    }
}

#[test]
fn a_picture_plugin_that_fails_a_page_says_the_same_in_the_host_and_the_app() {
    failing_page_agrees("image", "turn.png", true, false);
    failing_page_agrees("image", "turn.png", false, true);
    failing_page_agrees("image", "turn.png", false, false);
}

#[test]
fn an_svg_plugin_that_fails_a_page_says_the_same_in_the_host_and_the_app() {
    failing_page_agrees("svg", "turn.svg", true, false);
    failing_page_agrees("svg", "turn.svg", false, true);
    failing_page_agrees("svg", "turn.svg", false, false);
}

#[test]
fn a_pdf_plugin_that_fails_a_page_says_the_same_in_the_host_and_the_app() {
    failing_page_agrees("pdf", "turn.pdf", true, false);
    failing_page_agrees("pdf", "turn.pdf", false, true);
    failing_page_agrees("pdf", "turn.pdf", false, false);
}

#[test]
fn a_video_plugin_that_fails_a_page_says_the_same_in_the_host_and_the_app() {
    failing_page_agrees("video", "turn.mp4", true, false);
    failing_page_agrees("video", "turn.mp4", false, true);
    failing_page_agrees("video", "turn.mp4", false, false);
}

/// The first page of the plugin `id` fails and `J` turns to a second page that draws: the host
/// and the app end on the same cells (the page keys act over the failed first page, whichever
/// kind of picture the plugin shows).
fn first_page_failed_then_turned_agrees(id: &'static str, file: &'static str) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    std::fs::write(dir.join("turn.svg"), "<svg>junk-body</svg>").unwrap();
    std::fs::write(dir.join("turn.pdf"), "%PDF junk").unwrap();
    std::fs::write(
        dir.join("turn.mp4"),
        [&[0, 0, 0, 0x18][..], b"ftypmp42\0\0\0\0mp42isom"].concat(),
    )
    .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    let root = canon(&dir);
    let mk = || {
        let mut p =
            TurnPlugin::new(root.join("a0.gif"), Some(root.join("green.png"))).of_kind(id, file);
        p.first_fails = true;
        p.then = "J";
        p.end_on = Some(GREEN);
        p
    };
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join(file), mk(), Until::Nothing);
    let green = |cells: &[(String, RColor, RColor)]| {
        cells
            .iter()
            .filter(|c| near(c.1, GREEN) || near(c.2, GREEN))
            .count()
    };
    assert!(
        green(&h.cells) >= 8,
        "{id}: the host turned to page 2\n{}",
        h.rows.join("\n")
    );
    assert!(
        green(&a.cells) >= 8,
        "{id}: the app turned to page 2\n{}",
        a.rows.join("\n")
    );
    assert_eq!(h.cells, a.cells, "{id}: page 2 is drawn alike");
}

#[test]
fn j_turns_from_a_failed_first_page_alike_for_image_svg_pdf_and_video() {
    first_page_failed_then_turned_agrees("image", "turn.png");
    first_page_failed_then_turned_agrees("svg", "turn.svg");
    first_page_failed_then_turned_agrees("pdf", "turn.pdf");
    first_page_failed_then_turned_agrees("video", "turn.mp4");
}

/// A page of a picture plugin that fails and is shown in the picture's own frame (the host's
/// `Picture` words), after the keys `keys` and, with `help`, the `?` help: the footer's hints
/// and the help's picture rows of the app are those of the host, and the page turn is named by
/// both (the first page failing or not).
fn failed_picture_chrome_agrees(first_fails: bool, still_before: bool, help: bool) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    let root = canon(&dir);
    let mk = || {
        let gif = if still_before { "green.png" } else { "a0.gif" };
        let mut p = TurnPlugin::new(root.join(gif), None);
        p.first_fails = first_fails;
        p.before = if still_before { GREEN } else { RED };
        p.then = if help { "?" } else { "" };
        p
    };
    let what = format!("first_fails={first_fails} still_before={still_before} help={help}");
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join("turn.png"), mk(), Until::Failure);
    picture_chrome_agrees(&what, &h, &a, help, true);
}

/// The footer (or, with `help`, the `?` help) of a picture's own frame that holds a failure: what
/// the app names is what the host names.
fn picture_chrome_agrees(what: &str, h: &Shot, a: &Shot, help: bool, paged: bool) {
    if !help {
        let (hh, ah) = (
            hints(h.rows.last().expect("footer")),
            hints(a.rows.last().expect("footer")),
        );
        // The app's footer ends with the title.
        let (ah, _title) = ah.split_at(ah.len() - 1);
        assert!(ah.len() >= 2, "{what}: {ah:?}");
        // No text is on screen to scroll: no motion is named (the hint is there iff the key acts).
        for (side, hs) in [("host", &hh), ("app", &ah.to_vec())] {
            assert!(
                !hs.iter().any(|t| t.ends_with(":scroll")
                    || t.ends_with(":ends")
                    || t.ends_with(":hscroll")
                    || t.ends_with(":line ends")),
                "{what}: the {side} footer names a motion that does nothing\n{hs:?}"
            );
        }
        for t in ah {
            assert!(
                hh.contains(t) || t == "q:quit",
                "{what}: the app footer has `{t}` the host lacks\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        for t in hh.iter().filter(|t| !ah.contains(t)) {
            assert!(
                TEXT_HOST_ONLY.contains(&t.as_str()),
                "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        return;
    }
    let (py, px) = h
        .rows
        .iter()
        .enumerate()
        .find_map(|(y, r)| {
            r.find("┌ Help")
                .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
        })
        .unwrap_or_else(|| panic!("{what}: the help panel\n{}", h.rows.join("\n")));
    let hl = help_lines(&h.rows[py + 1..], Some(px));
    let al = help_lines(&a.rows, None);
    let has = |l: &[String], p: &str| l.iter().any(|r| r.starts_with(p));
    // No text is on screen to scroll: no motion row is named by either.
    for (side, l) in [("host", &hl), ("app", &al)] {
        for motion in ["j / k", "g / G", "h / l", "0 / $", "Ctrl-f", "C-f"] {
            assert!(
                !has(l, motion),
                "{what}: the {side} help names `{motion}`, which scrolls nothing here\n{l:#?}"
            );
        }
    }
    // The page turn is named by both (a paged document) or by neither.
    assert_eq!(has(&hl, "J / K"), paged, "{what}: host\n{hl:#?}");
    assert_eq!(has(&al, "J / K"), paged, "{what}: app\n{al:#?}");
    // The sections the app draws for a picture are the host's rows.
    for row in al.iter().filter(|r| !r.is_empty()) {
        if row.starts_with("q / Esc") || row.starts_with("? ") || row.starts_with("Q ") {
            continue;
        }
        assert!(
            hl.iter().any(|r| same_row(r, row)),
            "{what}: the app's help row `{row}` is not in the host's\nhost: {hl:#?}\napp: {al:#?}"
        );
    }
}

#[test]
fn the_footer_and_the_help_of_a_failed_picture_page_agree_with_the_host() {
    for (first_fails, still_before) in [(false, true), (false, false), (true, false)] {
        for help in [false, true] {
            failed_picture_chrome_agrees(first_fails, still_before, help);
        }
    }
}

/// A body of `n` lines, each saying its number, in an SVG file that is not valid.
fn long_broken_svg(n: usize) -> String {
    let lines: Vec<String> = (0..n).map(|i| format!("junk-line-{i:03}")).collect();
    format!("<svg>\n{}\n</svg>", lines.join("\n"))
}

/// A broken SVG longer than the screen, scrolled by `keys`: the rows are those the host shows.
fn broken_svg_scrolled_agrees(first_fails: bool, still_before: bool, keys: &'static str) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
    let root = canon(&dir);
    let mk = || {
        let gif = if still_before { "green.png" } else { "a0.gif" };
        let mut p = TurnPlugin::new(root.join(gif), None).of_kind("svg", "turn.svg");
        p.first_fails = first_fails;
        p.before = if still_before { GREEN } else { RED };
        p.then = keys;
        p
    };
    let what = format!("first_fails={first_fails} still_before={still_before} keys={keys:?}");
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join("turn.svg"), mk(), Until::Text("junk-line-000"));
    let (hr, ar) = (content_rows(&h.rows), content_rows(&a.rows));
    assert!(
        hr.iter().any(|r| r.contains("junk-line-")),
        "{what}: host shows the body\n{}",
        h.rows.join("\n")
    );
    if keys.is_empty() || keys.ends_with('g') {
        // At the top (never scrolled, or back at the top).
        assert!(hr.iter().any(|r| r.contains("junk-line-000")), "{what}");
        assert!(
            ar.iter().any(|r| r.contains("junk-line-000")),
            "{what}: app"
        );
    } else {
        // The first lines have scrolled away: the screen moved on in both.
        assert!(
            !hr.iter().any(|r| r.contains("junk-line-000")),
            "{what}: host scrolled\n{}",
            h.rows.join("\n")
        );
        assert!(
            !ar.iter().any(|r| r.contains("junk-line-000")),
            "{what}: app scrolled\n{}",
            a.rows.join("\n")
        );
    }
    // The app has no border, so its window is taller than the host's by a row or two: the
    // rows both show are compared, from the top (or from the bottom, at the end of the file).
    let n = hr.len().min(ar.len());
    assert!(n >= 15, "{what}: a screenful to compare");
    let at_end = keys.ends_with('G');
    let (hs, as_) = if at_end {
        (&hr[hr.len() - n..], &ar[ar.len() - n..])
    } else {
        (&hr[..n], &ar[..n])
    };
    assert_eq!(
        hs,
        as_,
        "{what}\nhost:\n{}\napp:\n{}",
        h.rows.join("\n"),
        a.rows.join("\n")
    );
}

#[test]
fn a_broken_svg_longer_than_the_screen_scrolls_as_in_the_host() {
    for (first_fails, still_before) in [(true, false), (false, true), (false, false)] {
        broken_svg_scrolled_agrees(first_fails, still_before, "");
        broken_svg_scrolled_agrees(first_fails, still_before, "j j j j j j j j j j j j j j j");
        broken_svg_scrolled_agrees(first_fails, still_before, "G");
        broken_svg_scrolled_agrees(first_fails, still_before, "G g");
    }
}

// ---- a plugin that declares nothing and fails -----------------------------------------------------

/// A plugin that declares nothing (`Unavailable::Message`) and fails every view with its own
/// words: the host and the app both say those words and the path.
struct SaysNo;

impl Plugin for SaysNo {
    fn manifest(&self) -> Manifest {
        Manifest::new("says-no", "0.0.1").bind(Bind::files(["*.sayno"]))
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(SaysNoSession))
    }
}

struct SaysNoSession;

impl Session for SaysNoSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Err(Failure::corrupt(t!(en: "the file said no on purpose")))
    }
}

#[test]
fn a_plugin_that_declares_nothing_and_fails_says_the_same_in_the_host_and_the_app() {
    let dir = fixtures();
    std::fs::write(dir.join("x.sayno"), "body").unwrap();
    let root = canon(&dir);
    let marker = crate::plugins::test_plugins::register("says-no", Arc::new(SaysNo));
    let mut s = Sim::with_config_sized(
        &root,
        {
            let mut c = crate::plugins::test_plugins::marker_config(&marker);
            c.ui.lang = "en".into();
            c
        },
        W,
        HOST_H,
    );
    s.select("x.sayno");
    s.enter();
    s.draw();
    let h = super::image_parity::shot(&s.term);
    let a = app_msg_rows(&root.join("x.sayno"));
    let (hr, ar) = (content_rows(&h.rows), content_rows(&a));
    assert!(
        hr.iter().any(|r| r.contains("the file said no on purpose")),
        "{}",
        h.rows.join("\n")
    );
    assert!(hr.iter().any(|r| r.contains("x.sayno")), "{hr:?}");
    assert_eq!(
        hr,
        ar,
        "host:\n{}\napp:\n{}",
        h.rows.join("\n"),
        a.join("\n")
    );
}

fn app_msg_rows(path: &std::path::Path) -> Vec<String> {
    let mut opened = open_file(vec![Arc::new(SaysNo)], path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, APP_H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        plugin_id: "says-no".into(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
    };
    let mut events = OnceShowing {
        screen: Arc::clone(&screen),
        want: Until::Text("said no"),
        seen: false,
        keys: std::collections::VecDeque::new(),
        started: Instant::now(),
    };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        None,
    )
    .unwrap();
    let guard = screen.lock().unwrap();
    shot_of(guard.buffer()).rows
}

/// A plugin that fails with words that carry control characters (a tab, an escape sequence, a
/// bell): the host draws them through ratatui, which drops them. The app does the same.
struct SaysCtl;

impl Plugin for SaysCtl {
    fn manifest(&self) -> Manifest {
        Manifest::new("says-ctl", "0.0.1").bind(Bind::files(["*.sayctl"]))
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(SaysCtlSession))
    }
}

struct SaysCtlSession;

impl Session for SaysCtlSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Err(Failure::corrupt(
            t!(en: "left\tright \u{1b}[31mred\u{7}bell \u{9b}c1 tail"),
        ))
    }
}

/// A failure message with control characters, and a full-width file name in the path under it:
/// the rows of the host and the app are the same.
#[test]
fn control_characters_and_a_full_width_path_in_a_failure_message_are_drawn_as_in_the_host() {
    let dir = fixtures();
    let name = "全角のファイル名.sayctl";
    std::fs::write(dir.join(name), "body").unwrap();
    let root = canon(&dir);
    let marker = crate::plugins::test_plugins::register("says-ctl", Arc::new(SaysCtl));
    let mut s = Sim::with_config_sized(
        &root,
        {
            let mut c = crate::plugins::test_plugins::marker_config(&marker);
            c.ui.lang = "en".into();
            c
        },
        W,
        HOST_H,
    );
    s.select(name);
    s.enter();
    s.draw();
    let h = super::image_parity::shot(&s.term);

    let path = root.join(name);
    let mut opened =
        open_file(vec![Arc::new(SaysCtl)], &path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, APP_H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        plugin_id: "says-ctl".into(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
    };
    let mut events = OnceShowing {
        screen: Arc::clone(&screen),
        want: Until::Text("tail"),
        seen: false,
        keys: std::collections::VecDeque::new(),
        started: Instant::now(),
    };
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        None,
    )
    .unwrap();
    let a = shot_of(screen.lock().unwrap().buffer()).rows;
    let (hr, ar) = (content_rows(&h.rows), content_rows(&a));
    assert!(
        hr.iter()
            .any(|r| r.contains("redbell") && r.contains("tail")),
        "{}",
        h.rows.join("\n")
    );
    assert!(
        hr.iter().any(|r| r.contains(name)),
        "the path is shown whole: {hr:?}"
    );
    assert_eq!(
        hr,
        ar,
        "host:\n{}\napp:\n{}",
        h.rows.join("\n"),
        a.join("\n")
    );
}

// ---- the footer and the help of a failed page shown as text ------------------------------------

/// The footer hints of the host's text view that the app does not have: the app names fewer
/// keys of a text (it has no tree, no search, no editor).
const TEXT_HOST_ONLY: &[&str] = &[
    "/:search",
    "F:FOLLOW",
    "C-n/C-p:next/prev file",
    "q:back",
    "e:edit",
    "o:open in app",
    "g/G:ends",
    "h/l:hscroll",
    "0/$:line ends",
    "[/]:tab",
    ",:view",
    "C-f/C-b/PgDn/PgUp:page",
];

/// The rows of the text section of a help: from its heading to the blank row.
fn text_section(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| l == "Preview: text / Markdown")
        .unwrap_or_else(|| panic!("the text section in {:#?}", &lines[..lines.len().min(12)]));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.is_empty() && !l.starts_with('│'))
        .cloned()
        .collect()
}

/// A broken SVG as a document of two pages (or of none: `paged` false) whose page 1 is a picture
/// before it (`still_before`: a still, else the animation) or fails at once (`first_fails`),
/// after the presses `keys`: the footer's hints and, with `help`, the rows of the `?` help the
/// app names are those the host names. Returns the number of hints and rows compared, and
/// whether the host named the page turn.
fn failed_text_chrome_agrees(
    paged: bool,
    first_fails: bool,
    still_before: bool,
    keys: &'static str,
    help: bool,
) -> (usize, bool) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
    let root = canon(&dir);
    let spec: &'static str = Box::leak(format!("{keys} ?").into_boxed_str());
    let mk = || {
        let gif = if still_before { "green.png" } else { "a0.gif" };
        let mut p = TurnPlugin::new(root.join(gif), None).of_kind("svg", "turn.svg");
        p.first_fails = first_fails;
        p.paged = paged;
        p.before = if still_before { GREEN } else { RED };
        p.then = if help { spec } else { keys };
        p
    };
    let what = format!(
        "paged={paged} first_fails={first_fails} still_before={still_before} [{keys}] help={help}"
    );
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join("turn.svg"), mk(), Until::Text("junk-line-000"));
    text_chrome_agrees(&what, &h, &a, help)
}

/// A paged document whose page 2 is a broken SVG longer than the screen (the picture is handed
/// over and fails to decode, the plugin's `view` does not fail): after `J` and `keys` the rows,
/// the footer and the help are the host's.
fn paged_decode_failure_agrees(kind: &'static str, keys: &'static str, help: bool) {
    let dir = fixtures();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    // The broken page is the document's own file for an SVG (its text is what the failure shows).
    let (file, second) = if kind == "svg" {
        std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
        ("turn.svg", "turn.svg")
    } else {
        std::fs::write(dir.join("page2.png"), b"not a png at all").unwrap();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .save(dir.join("turn.png"))
            .unwrap();
        ("turn.png", "page2.png")
    };
    let root = canon(&dir);
    let spec: &'static str = Box::leak(format!("{keys} ?").into_boxed_str());
    let mk = || {
        let mut p =
            TurnPlugin::new(root.join("green.png"), Some(root.join(second))).of_kind(kind, file);
        p.before = GREEN;
        p.then = if help { spec } else { keys };
        p
    };
    let what = format!("kind={kind} [{keys}] help={help}");
    let until = if kind == "svg" {
        Until::Text("junk-line-000")
    } else {
        Until::Failure
    };
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join(file), mk(), until);
    if kind == "svg" {
        if !help {
            let (hr, ar) = (content_rows(&h.rows), content_rows(&a.rows));
            let n = hr.len().min(ar.len());
            assert!(n >= 15, "{what}: a screenful to compare");
            let (hs, as_) = if keys.ends_with('G') {
                (&hr[hr.len() - n..], &ar[ar.len() - n..])
            } else {
                (&hr[..n], &ar[..n])
            };
            assert_eq!(
                hs,
                as_,
                "{what}\nhost:\n{}\napp:\n{}",
                h.rows.join("\n"),
                a.rows.join("\n")
            );
        }
        let (compared, _) = text_chrome_agrees(&what, &h, &a, help);
        assert!(compared > 0 || help, "{what}: something was compared");
    } else {
        if !help {
            assert!(
                failure_line(&h.rows).is_some(),
                "{what}\n{}",
                h.rows.join("\n")
            );
            assert_eq!(failure_line(&h.rows), failure_line(&a.rows), "{what}");
        }
        picture_chrome_agrees(&what, &h, &a, help, true);
    }
}

#[test]
fn a_page_that_fails_to_decode_in_a_paged_document_is_shown_as_in_the_host() {
    for kind in ["svg", "image"] {
        for keys in ["", "j j j j j j j j j j j j j j j", "j j j G"] {
            if kind == "image" && !keys.is_empty() {
                continue;
            }
            for help in [false, true] {
                paged_decode_failure_agrees(kind, keys, help);
            }
        }
    }
}

/// Two pages that both fail to draw, the document being a long broken SVG: after `keys` (a page
/// turn among them) the rows are those the host shows, so the scroll goes on, or restarts, alike.
fn turning_between_failed_text_pages_agrees(keys: &'static str) {
    let dir = fixtures();
    std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
    let root = canon(&dir);
    let mk = || {
        let mut p = TurnPlugin::new(root.join("a0.gif"), None).of_kind("svg", "turn.svg");
        p.first_fails = true;
        p.then = keys;
        p
    };
    let h = host_turn_shot(&root, mk());
    let a = app_turn_shot(&root.join("turn.svg"), mk(), Until::Text("junk-line-000"));
    let (hr, ar) = (content_rows(&h.rows), content_rows(&a.rows));
    let n = hr.len().min(ar.len());
    assert!(n >= 15, "[{keys}] a screenful to compare");
    if keys != "J" {
        // The scroll is not reset by the page turn: the first lines are gone in both.
        assert!(
            !hr.iter().any(|r| r.contains("junk-line-000")),
            "[{keys}] host scrolled"
        );
        assert!(
            !ar.iter().any(|r| r.contains("junk-line-000")),
            "[{keys}] app scrolled"
        );
    }
    // The app has no border: after `G` the rows are compared from the bottom.
    let (hs, as_) = if keys.contains('G') {
        (&hr[hr.len() - n..], &ar[ar.len() - n..])
    } else {
        (&hr[..n], &ar[..n])
    };
    assert_eq!(
        hs,
        as_,
        "[{keys}]\nhost:\n{}\napp:\n{}",
        h.rows.join("\n"),
        a.rows.join("\n")
    );
}

#[test]
fn a_page_turn_between_failed_text_pages_scrolls_as_in_the_host() {
    let many = "j j j j j j j j j j j j j j j";
    for keys in [
        "J",
        &*Box::leak(format!("{many} J").into_boxed_str()),
        &*Box::leak(format!("{many} J K").into_boxed_str()),
        "G J",
        &*Box::leak(format!("J {many}").into_boxed_str()),
    ] {
        turning_between_failed_text_pages_agrees(keys);
    }
}

/// A [`TurnPlugin`] that records the page of every `view` it is asked for.
struct CountedPlugin {
    inner: TurnPlugin,
    log: Arc<Mutex<Vec<usize>>>,
}

struct CountedSession {
    inner: Box<dyn Session>,
    log: Arc<Mutex<Vec<usize>>>,
}

impl Plugin for CountedPlugin {
    fn manifest(&self) -> Manifest {
        self.inner.manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(CountedSession {
            inner: self.inner.open(input, cx)?,
            log: Arc::clone(&self.log),
        }))
    }
}

impl Session for CountedSession {
    fn layout(&self) -> Layout {
        self.inner.layout()
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.log.lock().unwrap().push(req.page.unwrap_or(0));
        self.inner.view(req, cx)
    }
}

/// Keys once the screen shows `want`; `at_first_key` runs before the first one, `at_end` after
/// the last (the app has handled every key by then).
struct Watched {
    screen: Arc<Mutex<TestBackend>>,
    want: Until,
    seen: bool,
    keys: std::collections::VecDeque<AppEvent>,
    started: Instant,
    at_first_key: Option<Box<dyn FnOnce()>>,
    at_end: Option<Box<dyn FnOnce()>>,
}

impl EventSource for Watched {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if !self.seen {
            self.seen = self.want.met(self.screen.lock().unwrap().buffer());
        }
        if !self.seen {
            assert!(
                self.started.elapsed() < Duration::from_secs(20),
                "never got there"
            );
            std::thread::sleep(wait.unwrap_or_default().min(Duration::from_millis(2)));
            return Next::Timeout;
        }
        if let Some(f) = self.at_first_key.take() {
            f();
        }
        match self.keys.pop_front() {
            Some(k) => Next::Event(k),
            None => {
                if let Some(f) = self.at_end.take() {
                    f();
                }
                Next::End
            }
        }
    }
}

fn key_events(spec: &str) -> std::collections::VecDeque<AppEvent> {
    presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect()
}

/// Runs the app over `plugin` bound to `path`, with or without pictures, the keys sent once the
/// screen shows `want`. Returns the final screen.
#[allow(clippy::too_many_arguments)]
fn run_watched(
    plugin: CountedPlugin,
    path: &std::path::Path,
    pictures: bool,
    want: Until,
    spec: &str,
    at_first_key: Option<Box<dyn FnOnce()>>,
    at_end: Option<Box<dyn FnOnce()>>,
) -> Shot {
    let (unavailable, kind) = (plugin.inner.unavailable.clone(), plugin.inner.kind);
    let mut opened = open_file(vec![Arc::new(plugin)], path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, APP_H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable,
        plugin_id: kind.into(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
    };
    let mut events = Watched {
        screen: Arc::clone(&screen),
        want,
        seen: false,
        keys: key_events(spec),
        started: Instant::now(),
        at_first_key,
        at_end,
    };
    let pics =
        pictures.then(|| Pictures::new(Picker::halfblocks(), "turn-parity", &opened.path, 800));
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        pics,
    )
    .unwrap();
    assert!(events.keys.is_empty(), "the app ended early: {spec}");
    let guard = screen.lock().unwrap();
    shot_of(guard.buffer())
}

/// Scrolling the text of a page that failed to draw asks the plugin for nothing and reads no file:
/// the plugin is asked once, the file is gone before the first key, and the text still scrolls.
#[test]
fn scrolling_a_failed_text_page_does_not_ask_the_plugin_or_read_the_file_again() {
    let dir = fixtures();
    std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
    let root = canon(&dir);
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut inner = TurnPlugin::new(root.join("a0.gif"), None).of_kind("svg", "turn.svg");
    inner.first_fails = true;
    let plugin = CountedPlugin {
        inner,
        log: Arc::clone(&log),
    };
    let (before, after) = (Arc::new(Mutex::new(0)), Arc::clone(&log));
    let (b2, file) = (Arc::clone(&before), root.join("turn.svg"));
    let l2 = Arc::clone(&log);
    let many = "j j j j j j j j j j j j j j j j j j j j j j j j j j j j j j j";
    let a = run_watched(
        plugin,
        &root.join("turn.svg"),
        true,
        Until::Text("junk-line-000"),
        &format!("{many} g {many} G"),
        Some(Box::new(move || {
            *b2.lock().unwrap() = l2.lock().unwrap().len();
            std::fs::remove_file(&file).unwrap();
        })),
        None,
    );
    let (b, n) = (*before.lock().unwrap(), after.lock().unwrap().len());
    assert!(b >= 1, "the plugin was asked for the page once at least");
    assert_eq!(
        n,
        b,
        "scrolling asked the plugin again: {:?}",
        after.lock().unwrap()
    );
    let rows = content_rows(&a.rows);
    assert!(
        rows.iter().any(|r| r.contains("junk-line-089")),
        "the kept text scrolled to its end although the file is gone:\n{}",
        a.rows.join("\n")
    );
}

/// With no pictures (no graphics backend) a paged document's page keys do nothing and are not
/// named: the plugin is never asked for page 2, and the footer does not name the page turn.
#[test]
fn without_pictures_the_page_keys_do_nothing_and_are_not_named() {
    let dir = fixtures();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(dir.join("turn.png"))
        .unwrap();
    image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 255, 0, 255]))
        .save(dir.join("green.png"))
        .unwrap();
    let root = canon(&dir);
    for (kind, file, first_fails) in [("image", "turn.png", false), ("svg", "turn.svg", true)] {
        std::fs::write(dir.join("turn.svg"), long_broken_svg(90)).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut inner = TurnPlugin::new(root.join("green.png"), Some(root.join("green.png")))
            .of_kind(kind, file);
        inner.first_fails = first_fails;
        let plugin = CountedPlugin {
            inner,
            log: Arc::clone(&log),
        };
        let a = run_watched(
            plugin,
            &root.join(file),
            false,
            Until::Nothing,
            "J K J",
            None,
            None,
        );
        assert!(
            log.lock().unwrap().iter().all(|p| *p == 0),
            "{kind}: a page key asked for another page: {:?}",
            log.lock().unwrap()
        );
        let foot = a.rows.last().unwrap();
        assert!(
            !foot.contains("J/K"),
            "{kind}: the footer names the page turn: {foot}"
        );
    }
}

/// On a failed picture frame j/k do nothing visible in either (the host draws `j/k:scroll` there,
/// a hint for a key that does not act: reported, not fixed): the screens stay as they were.
#[test]
fn j_and_k_leave_a_failed_picture_frame_alone_in_both() {
    let dir = fixtures();
    let root = canon(&dir);
    let h0 = host_shot_at(&root, "junk.gif", HOST_H, "", true, None);
    let h1 = host_shot_at(&root, "junk.gif", HOST_H, "j j k j", true, None);
    let a0 = app_gif_shot(&root.join("junk.gif"), APP_H, "", Until::Failure, true);
    let a1 = app_gif_shot(
        &root.join("junk.gif"),
        APP_H,
        "j j k j",
        Until::Failure,
        true,
    );
    assert_eq!(content_rows(&h0.rows), content_rows(&h1.rows), "host");
    assert_eq!(content_rows(&a0.rows), content_rows(&a1.rows), "app");
}

/// The footer (or, with `help`, the `?` help) of the text of a failed page: what the app names is
/// what the host names. Returns the number of hints and rows compared, and whether both named
/// the page turn.
fn text_chrome_agrees(what: &str, h: &Shot, a: &Shot, help: bool) -> (usize, bool) {
    let mut compared = 0;
    if !help {
        let (hh, ah) = (
            hints(h.rows.last().expect("footer")),
            hints(a.rows.last().expect("footer")),
        );
        // The app's footer ends with where the text is and the title.
        assert!(ah.len() > 3, "{what}: {ah:?}");
        let (ah, at) = ah.split_at(ah.len() - 2);
        assert!(
            at[0].starts_with("line "),
            "{what}: the app says where the text is: {at:?}"
        );
        for t in ah {
            assert!(
                hh.contains(t) || t == "q:quit",
                "{what}: the app footer has `{t}` the host lacks\nhost: {hh:?}\napp: {ah:?}"
            );
            compared += 1;
        }
        for t in hh.iter().filter(|t| !ah.contains(t)) {
            assert!(
                TEXT_HOST_ONLY.contains(&t.as_str()),
                "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        return (compared, false);
    }
    let (py, px) = h
        .rows
        .iter()
        .enumerate()
        .find_map(|(y, r)| {
            r.find("┌ Help")
                .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
        })
        .unwrap_or_else(|| panic!("{what}: the help panel\n{}", h.rows.join("\n")));
    let hs = text_section(&help_lines(&h.rows[py + 1..], Some(px)));
    let asec = text_section(&help_lines(&a.rows, None));
    for row in &asec {
        if row.starts_with("q / Esc") || row.starts_with("? ") || row.starts_with("Q ") {
            continue;
        }
        assert!(
            hs.iter().any(|r| same_row(r, row)),
            "{what}: the app's help row `{row}` is not in the host's\nhost: {hs:#?}\napp: {asec:#?}"
        );
        compared += 1;
    }
    // The page turn is named by both or by neither.
    let turn = |rows: &[String]| rows.iter().any(|r| r.starts_with("J / K"));
    assert_eq!(
        turn(&hs),
        turn(&asec),
        "{what}\nhost: {hs:#?}\napp: {asec:#?}"
    );
    (compared, turn(&hs) && turn(&asec))
}

#[test]
fn the_footer_and_the_help_of_a_failed_page_shown_as_text_agree_with_the_host() {
    let (mut compared, mut turns, mut plain) = (0, 0, 0);
    // A page that failed after a still or an animation, and a first page that failed.
    for (first_fails, still_before) in [(false, true), (false, false), (true, false)] {
        for keys in ["", "j j j", "G", "G g", "j j j J"] {
            for help in [false, true] {
                let (n, turn) =
                    failed_text_chrome_agrees(true, first_fails, still_before, keys, help);
                compared += n;
                turns += usize::from(turn);
            }
        }
    }
    // A document with no pages: no page turn in the help.
    for keys in ["", "j j j", "G"] {
        for help in [false, true] {
            let (n, turn) = failed_text_chrome_agrees(false, true, false, keys, help);
            assert!(!turn);
            compared += n;
            plain += 1;
        }
    }
    assert!(compared >= 100, "{compared}");
    assert_eq!(turns, 3 * 5, "the paged help names the page turn");
    assert_eq!(plain, 6);
}

// ---- a page the real plugin hands over and the draw fails ---------------------------------------
//
// The plugins above fail in `view`. The real ones (`konoma_media`) answer `view` with a picture and
// fail later, when it is decoded or rasterised: the screen, the footer and the help must be the
// same as for a `view` that failed, because the host decides them from the plugin's declared
// `unavailable` whichever step failed.

/// What the app shows for `path` opened by the real plugin `plugin` (id `id`), after the keys
/// `spec`, once the screen shows `until`.
fn app_real_shot(
    plugin: Arc<dyn Plugin>,
    id: &'static str,
    path: &std::path::Path,
    spec: &str,
    until: Until,
) -> Shot {
    let mut opened = open_file(vec![plugin], path, "en", TermCaps::new()).expect("opens");
    let screen = Arc::new(Mutex::new(TestBackend::new(W, APP_H)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        plugin_id: id.into(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
    };
    let keys = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = OnceShowing {
        screen: Arc::clone(&screen),
        want: until,
        seen: false,
        keys,
        started: Instant::now(),
    };
    let pics = Pictures::new(Picker::halfblocks(), id, &opened.path, 800);
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        Some(pics),
    )
    .unwrap();
    assert!(events.keys.is_empty(), "the app ended early: {spec}");
    let guard = screen.lock().unwrap();
    shot_of(guard.buffer())
}

/// A valid SVG of `n` rows, each with its number, cut off by a start tag that never ends.
fn broken_svg_after(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"<rect id="svg-row-{i:03}" x="0" y="0" width="1" height="1"/>"#))
        .collect();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\n{}\n<g><broken",
        rows.join("\n")
    )
}

/// The rows of the text both screens show, from the top (or, after `G`, from the bottom): the
/// app has no border and no status row, so it shows a row or two more than the host.
fn same_text_rows(what: &str, h: &Shot, a: &Shot, at_end: bool) {
    let (hr, ar) = (content_rows(&h.rows), content_rows(&a.rows));
    let n = hr.len().min(ar.len());
    assert!(n >= 15, "{what}: a screenful to compare");
    let (hs, as_) = if at_end {
        (&hr[hr.len() - n..], &ar[ar.len() - n..])
    } else {
        (&hr[..n], &ar[..n])
    };
    assert_eq!(
        hs,
        as_,
        "{what}\nhost:\n{}\napp:\n{}",
        h.rows.join("\n"),
        a.rows.join("\n")
    );
}

#[test]
fn a_broken_svg_the_real_plugin_hands_over_is_shown_as_in_the_host() {
    let dir = fixtures();
    std::fs::write(dir.join("cut.svg"), broken_svg_after(80)).unwrap();
    let root = canon(&dir);
    let mut texts = 0;
    for keys in [
        "",
        "j j j",
        "j j j j j j j j j j j j j j j",
        "G",
        "G g",
        "j j j G",
    ] {
        for help in [false, true] {
            let spec = if help {
                format!("{keys} ?")
            } else {
                keys.to_string()
            };
            let what = format!("cut.svg [{spec}]");
            let h = host_shot_at(&root, "cut.svg", HOST_H, &spec, true, None);
            let a = app_real_shot(
                Arc::new(konoma_media::SvgPlugin),
                "svg",
                &root.join("cut.svg"),
                &spec,
                Until::Text("svg-row-000"),
            );
            if !help {
                // The reason first, then the file as text; scrolls like any text.
                assert!(
                    h.rows.iter().any(|r| r.contains("svg-row-")),
                    "{what}: the host shows the file\n{}",
                    h.rows.join("\n")
                );
                assert!(
                    a.rows.iter().any(|r| r.contains("svg-row-")),
                    "{what}: the app shows the file\n{}",
                    a.rows.join("\n")
                );
                same_text_rows(&what, &h, &a, keys.ends_with('G'));
                texts += 1;
            }
            text_chrome_agrees(&what, &h, &a, help);
        }
    }
    assert_eq!(texts, 6);
}

#[test]
fn a_broken_png_the_real_plugin_hands_over_is_shown_as_in_the_host() {
    let dir = fixtures();
    std::fs::write(
        dir.join("cut.png"),
        b"\x89PNG\r\n\x1a\nnot really a picture",
    )
    .unwrap();
    let root = canon(&dir);
    for keys in ["", "j", "j k G g", "+ - 0"] {
        for help in [false, true] {
            let spec = if help {
                format!("{keys} ?")
            } else {
                keys.to_string()
            };
            let what = format!("cut.png [{spec}]");
            let h = host_shot_at(&root, "cut.png", HOST_H, &spec, true, None);
            let a = app_real_shot(
                Arc::new(konoma_media::ImagePlugin),
                "image",
                &root.join("cut.png"),
                &spec,
                Until::Failure,
            );
            if !help {
                let (hl, al) = (failure_line(&h.rows), failure_line(&a.rows));
                assert!(hl.is_some(), "{what}\n{}", h.rows.join("\n"));
                assert_eq!(hl, al, "{what}");
            }
            picture_chrome_agrees(&what, &h, &a, help, false);
        }
    }
}
