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
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
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
        // the channel to wait for): take what comes, then draw.
        let rx = s.media_rx.take().expect("with_media");
        while let Ok(res) = rx.recv_timeout(Duration::from_millis(500)) {
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
        s.draw();
        settle(&mut s, &mut req_rx);
    }
    STAND_IN.with(|s| *s.borrow_mut() = None);
    super::image_parity::shot(&s.term)
}

/// What the app shows after `J`, once it shows what the host ended on (`after`).
fn app_turn_shot(path: &std::path::Path, plugin: TurnPlugin, after: Until) -> Shot {
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
    let after = if then.is_empty() {
        after
    } else {
        Until::Nothing
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
    // (The page keys do not act in the app when the very first page failed, so it names none
    // there: a difference of behavior, not of the hints.)
    if !(first_fails && paged) {
        assert_eq!(
            turn(&hs),
            turn(&asec),
            "{what}\nhost: {hs:#?}\napp: {asec:#?}"
        );
    }
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
    assert_eq!(turns, 2 * 5, "the paged help names the page turn");
    assert_eq!(plain, 6);
}
