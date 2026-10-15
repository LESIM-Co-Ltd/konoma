//! A plugin's free-drawing view (`View::Canvas`) in the host, stage 2.5 / T2a (cells only).
//!
//! A stand-in plugin (`Painter`) draws a known pattern; the tests read the cells it leaves in the
//! `TestBackend` buffer, the `ViewReq` the plugin was asked with, the footer and the `?` help, and
//! drive the keys through the real `handle_key`. The plugin is registered the way a user's plugin
//! is (`plugins::test_plugins`), never in the shipped registry.

use super::*;
use crate::keymap::{Context, LeaderId};
use crate::ops::{self, menu::View as MenuView};
use crate::plugins::test_plugins::{self, Latch};
use crate::plugins::PluginAct;
use crossterm::event::KeyCode;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{CanvasView, Layout, ReprDecl};
use konoma_plugin::{ColorDepth, Style as PStyle};
use ratatui::style::{Color as RColor, Modifier as RModifier};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What the stand-in draws.
#[derive(Clone)]
enum Draw {
    /// A canvas of this fixed size with the test pattern (ASCII, wide characters, styles).
    Pattern { cols: u16, rows: u16 },
    /// The same pattern on a canvas filled with a blue background, so its edge can be seen.
    Filled { cols: u16, rows: u16 },
    /// A canvas of the size the host asks for; row 0 says that size.
    Follow,
    /// A canvas whose cell count does not match its size.
    Malformed,
    /// A wide character at column `at` of row 0 on a canvas of this size.
    WideAt { cols: u16, rows: u16, at: u16 },
}

/// What a `view` call was given.
#[derive(Clone, Debug)]
struct Seen {
    req: ViewReq,
    colors: ColorDepth,
}

#[derive(Clone)]
struct Painter {
    id: String,
    draw: Draw,
    /// Declare `shows(Canvas)`; when false, `shows(Text)` is declared and a canvas still comes back.
    declares_canvas: bool,
    /// Declare the `bump` action on `j` (a key the host keeps in a text view but not in a canvas)
    /// and `star` on `x`.
    acts: bool,
    /// Two representations, `one` and `two`.
    reprs: bool,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// When set, a `view` call waits for `hold` before it answers (a test that has several resizes
    /// out while one request is on the worker).
    hold_on: Arc<AtomicBool>,
    hold: Arc<Latch>,
    acts_run: Arc<AtomicUsize>,
}

impl Painter {
    fn new(id: &str, draw: Draw) -> Painter {
        Painter {
            id: id.to_string(),
            draw,
            declares_canvas: true,
            acts: false,
            reprs: false,
            seen: Arc::default(),
            hold_on: Arc::default(),
            hold: Arc::default(),
            acts_run: Arc::default(),
        }
    }

    fn with_acts(mut self) -> Painter {
        self.acts = true;
        self
    }

    fn with_reprs(mut self) -> Painter {
        self.reprs = true;
        self
    }

    fn undeclared(mut self) -> Painter {
        self.declares_canvas = false;
        self
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Plugin for Painter {
    fn manifest(&self) -> Manifest {
        let mut m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.pnt"]))
            .shows(if self.declares_canvas {
                ViewKind::Canvas
            } else {
                ViewKind::Text
            });
        if self.acts {
            m = m
                .action(
                    Act::new("bump")
                        .key("j")
                        .label(t!(en: "bump", ja: "加算"))
                        .help(t!(en: "Add one to the counter", ja: "カウンタを 1 増やす"))
                        .footer(1),
                )
                .action(
                    Act::new("star")
                        .key("x")
                        .label(t!(en: "star", ja: "星"))
                        .help(t!(en: "Draw a star", ja: "星を描く"))
                        .footer(2),
                );
        }
        m
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(PainterSession {
            p: self.clone(),
            n: 0,
            star: false,
        }))
    }
}

struct PainterSession {
    p: Painter,
    n: u32,
    star: bool,
}

/// The ASCII / CJK / style pattern every `Pattern` canvas carries:
/// row 0 `ABC` red bold, then a blue-background space; row 1 `日本語` in an RGB color;
/// row 2 `a日b` with an underline; row 3 the counter and the representation.
fn pattern(c: &mut CanvasView, n: u32, repr: &str, star: bool) {
    c.set_string(
        0,
        0,
        "ABC",
        PStyle::new().fg(Color::Red).add_modifier(Modifier::BOLD),
    );
    c.set_string(3, 0, " ", PStyle::new().bg(Color::Blue));
    c.set_string(0, 1, "日本語", PStyle::new().fg(Color::Rgb(10, 20, 30)));
    c.set_string(
        0,
        2,
        "a日b",
        PStyle::new().add_modifier(Modifier::UNDERLINED),
    );
    c.set_string(0, 3, &format!("n={n} {repr}"), PStyle::new());
    if star {
        c.set_string(0, 4, "*STAR*", PStyle::new().fg(Color::Yellow));
    }
}

impl Session for PainterSession {
    fn layout(&self) -> Layout {
        if self.p.reprs {
            Layout {
                reprs: vec![
                    ReprDecl::new("one", t!(en: "One", ja: "一")),
                    ReprDecl::new("two", t!(en: "Two", ja: "二")),
                ],
                default_repr: Some("one".into()),
                default_repr_without_pixels: Some("one".into()),
                pages: None,
            }
        } else {
            Layout::single()
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.p.seen.lock().unwrap().push(Seen {
            req: req.clone(),
            colors: cx.term.colors,
        });
        // The first answers (the open and the first measured frame) are never held.
        if self.p.hold_on.load(Ordering::SeqCst) {
            self.p.hold.wait();
        }
        let repr = req.repr.clone().unwrap_or_else(|| "one".to_string());
        let canvas = match self.p.draw.clone() {
            Draw::Pattern { cols, rows } => {
                let mut c = CanvasView::new(cols, rows);
                pattern(&mut c, self.n, &repr, self.star);
                c
            }
            Draw::Filled { cols, rows } => {
                let mut c = CanvasView::new(cols, rows);
                c.fill(
                    konoma_plugin::view::CellRect::new(0, 0, cols, rows),
                    PStyle::new().bg(Color::Blue),
                );
                pattern(&mut c, self.n, &repr, self.star);
                c
            }
            Draw::Follow => {
                let mut c = CanvasView::new(req.cols, req.rows);
                c.set_string(
                    0,
                    0,
                    &format!("size={}x{}", req.cols, req.rows),
                    PStyle::new(),
                );
                c
            }
            Draw::WideAt { cols, rows, at } => {
                let mut c = CanvasView::new(cols, rows);
                c.set_string(at, 0, "日", PStyle::new().fg(Color::Green));
                c
            }
            Draw::Malformed => {
                let mut c = CanvasView::new(4, 3);
                c.cells.pop();
                c
            }
        };
        Ok(View::Canvas(canvas))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.p.acts_run.fetch_add(1, Ordering::SeqCst);
        match action {
            "bump" => {
                self.n += 1;
                Ok(Outcome::Redraw)
            }
            "star" => {
                self.star = !self.star;
                Ok(Outcome::Redraw)
            }
            _ => Ok(Outcome::Unhandled),
        }
    }
}

// ---- helpers -----------------------------------------------------------------------------------

struct Opened {
    s: Sim,
    p: Painter,
    _dir: crate::test_support::TmpDir,
}

fn open_with(name: &str, p: Painter, w: u16, h: u16, lang: &str) -> Opened {
    let dir = sandbox(name);
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let marker = test_plugins::register(name, Arc::new(p.clone()));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = lang.into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, w, h);
    s.select("a.pnt");
    s.enter();
    // The first frame measured the area and asked the plugin again (without a worker the answer
    // lands inside that frame; the run loop would redraw on its arrival): draw the new canvas.
    s.draw();
    Opened { s, p, _dir: dir }
}

fn open(name: &str, draw: Draw) -> Opened {
    open_with(name, Painter::new(name, draw), 90, 26, "en")
}

/// The preview frame in the drawn buffer: (x, y, cols, rows) of the content inside the border,
/// found from the corner glyphs and not from the app's own numbers.
fn content_area(s: &Sim) -> (u16, u16, u16, u16) {
    let buf = s.term.backend().buffer();
    let (w, h) = (buf.area.width, buf.area.height);
    let mut tl = None;
    let mut br = None;
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

/// The buffer cell at (`x`, `y`) of the content area.
fn cell(s: &Sim, x: u16, y: u16) -> ratatui::buffer::Cell {
    let (ax, ay, _, _) = content_area(s);
    s.term.backend().buffer()[(ax + x, ay + y)].clone()
}

fn footer(s: &Sim) -> String {
    s.screen().lines().last().unwrap_or_default().to_string()
}

/// Let every background answer land (the async tests): apply results until nothing is out.
#[track_caller]
fn settle(s: &mut Sim) {
    for _ in 0..20 {
        if !s.app.is_media_loading() {
            return;
        }
        s.drain_plugin();
    }
    panic!("a plugin job is still out after 20 answers");
}

fn kitty(mut s: Sim) -> Sim {
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::mem::forget(rx);
    s.app.attach_image_backend(picker, tx);
    s
}

// ---- the cells ---------------------------------------------------------------------------------

#[test]
fn e2e_canvas_draws_ascii_wide_characters_and_styles_cell_for_cell() {
    let o = open("cv_cells", Draw::Pattern { cols: 20, rows: 6 });
    let s = &o.s;
    assert!(s.app.is_canvas_preview());
    assert_eq!(s.app.key_context(), Some(Context::Canvas));

    // Row 0: `ABC` in red and bold, then a space on a blue background.
    for (x, ch) in ["A", "B", "C"].iter().enumerate() {
        let c = cell(s, x as u16, 0);
        assert_eq!(c.symbol(), *ch);
        assert_eq!(c.fg, RColor::Red);
        assert!(c.modifier.contains(RModifier::BOLD), "bold at {x}");
    }
    let blue = cell(s, 3, 0);
    assert_eq!(blue.symbol(), " ");
    assert_eq!(blue.bg, RColor::Blue);

    // Row 1: three wide characters; each takes two columns (the left cell holds the symbol, the
    // right cell is the continuation and is left empty).
    for (i, ch) in ["日", "本", "語"].iter().enumerate() {
        let left = cell(s, (i * 2) as u16, 1);
        assert_eq!(left.symbol(), *ch, "wide character {i}");
        assert_eq!(left.fg, RColor::Rgb(10, 20, 30));
        let right = cell(s, (i * 2 + 1) as u16, 1);
        assert_eq!(right.symbol(), " ", "continuation of {i} draws nothing");
    }
    assert_eq!(cell(s, 6, 1).symbol(), " ", "nothing after the third");

    // Row 2: `a日b` = a, wide (2 columns), b.
    assert_eq!(cell(s, 0, 2).symbol(), "a");
    assert_eq!(cell(s, 1, 2).symbol(), "日");
    assert_eq!(cell(s, 3, 2).symbol(), "b");
    assert!(cell(s, 0, 2).modifier.contains(RModifier::UNDERLINED));
    assert!(cell(s, 1, 2).modifier.contains(RModifier::UNDERLINED));

    // Row 3: plain text with the default style (no color asked for = the terminal's own).
    assert_eq!(cell(s, 0, 3).symbol(), "n");
    assert_eq!(cell(s, 0, 3).fg, RColor::Reset);
    assert!(s.screen().contains("n=0 one"));
}

#[test]
fn e2e_canvas_keeps_the_frame_and_titles_it_with_the_file() {
    let o = open("cv_frame", Draw::Pattern { cols: 20, rows: 6 });
    let s = &o.s;
    s.see("a.pnt");
    let (ax, ay, aw, ah) = content_area(s);
    let buf = s.term.backend().buffer();
    assert_eq!(buf[(ax - 1, ay + 1)].symbol(), "│");
    assert_eq!(buf[(ax + aw, ay + 1)].symbol(), "│");
    assert_eq!(buf[(ax + 1, ay + ah)].symbol(), "─");
}

#[test]
fn e2e_canvas_larger_than_the_area_is_clipped_inside_the_frame() {
    // 200 x 80 on a 90 x 26 terminal: nothing may spill over the border, the bottom border or the
    // footer. The canvas is filled blue, so a spill would show.
    let o = open(
        "cv_clip",
        Draw::Filled {
            cols: 200,
            rows: 80,
        },
    );
    let s = &o.s;
    let (ax, ay, aw, ah) = content_area(s);
    let buf = s.term.backend().buffer();
    for y in 0..ah {
        assert_eq!(buf[(ax - 1, ay + y)].symbol(), "│", "left border, row {y}");
        assert_eq!(
            buf[(ax + aw, ay + y)].symbol(),
            "│",
            "right border, row {y}"
        );
        // (the pattern is drawn over the top left; the fill shows everywhere else)
        for x in 8..aw {
            assert_eq!(buf[(ax + x, ay + y)].bg, RColor::Blue, "({x},{y}) painted");
        }
    }
    for x in 0..aw {
        assert_eq!(
            buf[(ax + x, ay + ah)].symbol(),
            "─",
            "bottom border, col {x}"
        );
        assert_ne!(buf[(ax + x, ay + ah)].bg, RColor::Blue);
    }
    // The footer row below the frame is the footer's.
    assert!(!footer(s).is_empty());
    assert_ne!(buf[(ax, ay + ah + 1)].bg, RColor::Blue);
}

#[test]
fn e2e_canvas_never_draws_half_of_a_wide_character_at_the_clipped_edge() {
    // The canvas is wider than the area (100 > 88) and a wide character sits on the last visible
    // column: its right half would be outside, so it is not drawn at all.
    let o = open(
        "cv_clip_wide",
        Draw::WideAt {
            cols: 100,
            rows: 5,
            at: 87,
        },
    );
    let s = &o.s;
    let (_, _, aw, _) = content_area(s);
    assert_eq!(aw, 88);
    assert_eq!(
        cell(s, 87, 0).symbol(),
        " ",
        "the cut wide character is blank"
    );
    assert_eq!(cell(s, 87, 0).fg, RColor::Reset);
    // A wide character that fits whole next to it is drawn.
    let o = open(
        "cv_clip_wide_ok",
        Draw::WideAt {
            cols: 100,
            rows: 5,
            at: 86,
        },
    );
    assert_eq!(cell(&o.s, 86, 0).symbol(), "日");
    assert_eq!(cell(&o.s, 86, 0).fg, RColor::Green);
}

#[test]
fn e2e_canvas_smaller_than_the_area_leaves_the_rest_as_it_was() {
    let o = open("cv_small", Draw::Filled { cols: 10, rows: 5 });
    let s = &o.s;
    let (_, _, aw, ah) = content_area(s);
    for y in 0..ah {
        for x in 0..aw {
            let c = cell(s, x, y);
            if x < 10 && y < 5 {
                // (the pattern is drawn over the top left of the fill)
                if x >= 8 || y == 4 {
                    assert_eq!(c.bg, RColor::Blue, "inside ({x},{y})");
                }
            } else {
                assert_eq!(c.symbol(), " ", "outside ({x},{y})");
                assert_eq!(
                    c.bg,
                    RColor::Reset,
                    "outside ({x},{y}) keeps the background"
                );
                assert_eq!(c.fg, RColor::Reset);
            }
        }
    }
}

#[test]
fn e2e_canvas_a_zero_sized_canvas_draws_nothing_and_does_not_panic() {
    let o = open("cv_zero", Draw::Pattern { cols: 0, rows: 0 });
    assert!(o.s.app.is_canvas_preview());
    assert_eq!(cell(&o.s, 0, 0).symbol(), " ");
}

#[test]
fn e2e_canvas_survives_a_terminal_smaller_than_the_canvas() {
    let mut o = open("cv_tiny", Draw::Filled { cols: 30, rows: 10 });
    o.s.resize(12, 6);
    o.s.resize(3, 3);
    o.s.resize(90, 26);
    assert!(o.s.app.is_canvas_preview());
}

// ---- the request ------------------------------------------------------------------------------

#[test]
fn e2e_canvas_is_asked_with_the_real_size_of_the_area() {
    let o = open("cv_req", Draw::Follow);
    let (_, _, aw, ah) = content_area(&o.s);
    assert_eq!((aw, ah), (88, 22), "the preview area of a 90 x 26 terminal");
    let last = o.p.requests().last().cloned().expect("asked");
    assert_eq!((last.req.cols, last.req.rows), (aw, ah));
    // The canvas the plugin made at that size fills the area: its row 0 says so.
    assert!(o.s.screen().contains("size=88x22"), "{}", o.s.screen());
}

#[test]
fn e2e_canvas_request_has_no_cell_size_without_pixels_and_one_with_a_picture_backend() {
    // No image backend, and half blocks, draw no pixels: no cell size to give.
    let o = open("cv_cellpx_none", Draw::Follow);
    assert!(o.p.requests().iter().all(|r| r.req.cell_px.is_none()));

    let dir = sandbox("cv_cellpx_halfblocks");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let p = Painter::new("cv_cellpx_halfblocks", Draw::Follow);
    let marker = test_plugins::register("cv_cellpx_halfblocks", Arc::new(p.clone()));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26).with_picker();
    s.select("a.pnt");
    s.enter();
    assert!(
        p.requests().iter().all(|r| r.req.cell_px.is_none()),
        "half blocks draw no pixels"
    );

    // A kitty-graphics backend: the picker's font size.
    let dir = sandbox("cv_cellpx_kitty");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let p = Painter::new("cv_cellpx_kitty", Draw::Follow);
    let marker = test_plugins::register("cv_cellpx_kitty", Arc::new(p.clone()));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = kitty(Sim::with_config_sized(&canon(&dir), cfg, 90, 26));
    s.select("a.pnt");
    s.enter();
    let font = ratatui_image::picker::Picker::halfblocks().font_size();
    let last = p.requests().last().cloned().expect("asked");
    assert_eq!(last.req.cell_px, Some((font.width, font.height)));
}

#[test]
fn e2e_canvas_resize_asks_again_with_the_new_size_and_only_then() {
    let mut o = open("cv_resize", Draw::Follow);
    o.s.see("size=88x22");
    let before = o.p.requests().len();

    // The same size drawn again, and again: no request.
    o.s.draw();
    o.s.draw();
    assert_eq!(
        o.p.requests().len(),
        before,
        "an unchanged size asks nothing"
    );

    // A new size: one request with it, and the canvas follows after the next frame.
    o.s.resize(70, 20);
    let (_, _, aw, ah) = content_area(&o.s);
    assert_eq!((aw, ah), (68, 16));
    let reqs = o.p.requests();
    assert_eq!(reqs.len(), before + 1);
    assert_eq!(
        (reqs.last().unwrap().req.cols, reqs.last().unwrap().req.rows),
        (68, 16)
    );
    o.s.draw();
    o.s.see("size=68x16");
    assert_eq!(
        o.p.requests().len(),
        before + 1,
        "the answer is not asked again"
    );

    // Growing is the same.
    o.s.resize(100, 30);
    o.s.draw();
    o.s.see("size=98x26");
    assert_eq!(o.p.requests().len(), before + 2);
}

#[test]
fn e2e_canvas_a_drag_of_resizes_is_one_request_at_a_time_and_the_newest_size_wins() {
    let p = Painter::new("cv_drag", Draw::Follow);
    let dir = sandbox("cv_drag");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let marker = test_plugins::register("cv_drag", Arc::new(p.clone()));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26).with_async_plugins();
    s.select("a.pnt");
    s.enter();
    settle(&mut s);
    s.see("size=88x22");
    let before = p.requests().len();

    // The plugin holds the next answer; meanwhile the window is dragged through many sizes.
    p.hold_on.store(true, Ordering::SeqCst);
    s.resize(80, 24);
    for w in [78, 76, 74, 72, 70] {
        s.resize(w, 24);
    }
    // Only the first of those asked: the session is on the worker, and nothing is stacked up.
    p.hold.open();
    settle(&mut s);
    let reqs = p.requests();
    let added: Vec<_> = reqs[before..]
        .iter()
        .map(|r| (r.req.cols, r.req.rows))
        .collect();
    assert_eq!(
        added,
        vec![(78, 20), (68, 20)],
        "the first size of the drag, then the newest one: {added:?}"
    );
    s.see("size=68x20");
}

// ---- the colors ------------------------------------------------------------------------------

#[test]
fn color_depth_from_the_environment_values() {
    use crate::app::color_depth_from_for_test as d;
    assert_eq!(d(Some("truecolor"), Some("xterm")), ColorDepth::TrueColor);
    assert_eq!(d(Some("24bit"), None), ColorDepth::TrueColor);
    assert_eq!(d(Some("TrueColor"), Some("dumb")), ColorDepth::TrueColor);
    assert_eq!(d(None, Some("xterm-256color")), ColorDepth::Ansi256);
    assert_eq!(d(Some(""), Some("screen-256color")), ColorDepth::Ansi256);
    assert_eq!(d(None, Some("xterm-kitty")), ColorDepth::TrueColor);
    assert_eq!(d(None, Some("xterm-ghostty")), ColorDepth::TrueColor);
    assert_eq!(d(None, Some("wezterm")), ColorDepth::TrueColor);
    // A claim of true color wins over a 256-color name.
    assert_eq!(
        d(Some("truecolor"), Some("xterm-256color")),
        ColorDepth::TrueColor
    );
    // Nothing known promises the least.
    assert_eq!(d(None, None), ColorDepth::Ansi16);
    assert_eq!(d(None, Some("xterm")), ColorDepth::Ansi16);
    assert_eq!(d(None, Some("linux")), ColorDepth::Ansi16);
    assert_eq!(d(Some("yes"), Some("vt100")), ColorDepth::Ansi16);
}

#[test]
fn e2e_canvas_the_plugin_is_told_how_many_colors_the_terminal_shows() {
    for (depth, tag) in [
        (ColorDepth::Ansi16, "a16"),
        (ColorDepth::Ansi256, "a256"),
        (ColorDepth::TrueColor, "tc"),
    ] {
        let name = format!("cv_colors_{tag}");
        let dir = sandbox(&name);
        std::fs::write(dir.join("a.pnt"), b"x").unwrap();
        let p = Painter::new(&name, Draw::Follow);
        let marker = test_plugins::register(&name, Arc::new(p.clone()));
        let mut cfg = test_plugins::marker_config(&marker);
        cfg.ui.lang = "en".into();
        let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
        s.app.set_plugin_colors_for_test(depth);
        s.select("a.pnt");
        s.enter();
        let seen = p.requests();
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|r| r.colors == depth),
            "{depth:?}: {seen:?}"
        );
    }
}

// ---- refusing a bad canvas --------------------------------------------------------------------

#[test]
fn e2e_canvas_a_malformed_canvas_is_refused_with_a_message() {
    let o = open("cv_bad", Draw::Malformed);
    assert!(!o.s.app.is_canvas_preview());
    let f = o.s.app.plugin_failure().expect("a failure");
    assert!(
        f.message.en().contains("malformed canvas"),
        "{}",
        f.message.en()
    );
    assert!(f.message.en().contains("cv_bad"));
    assert!(f.message.en().contains("11 cells, expected 12"));
    o.s.see("the plugin drew a malformed canvas");
    o.s.see("cv_bad");
}

#[test]
fn e2e_canvas_a_malformed_canvas_is_refused_in_japanese_too() {
    let o = open_with(
        "cv_bad_ja",
        Painter::new("cv_bad_ja", Draw::Malformed),
        90,
        26,
        "ja",
    );
    let f = o.s.app.plugin_failure().expect("a failure");
    assert!(
        f.message
            .get("ja")
            .contains("プラグインの描いた表示の形式が正しくありません"),
        "{}",
        f.message.get("ja")
    );
    assert!(f.message.get("ja").contains("cv_bad_ja"));
}

#[test]
fn e2e_canvas_a_canvas_with_a_picture_outside_it_is_refused() {
    // The check is the API's (`CanvasView::check`): an image reaching outside the canvas.
    use konoma_plugin::view::{CanvasImage, CellRect, ImageData};
    struct OutOfBounds;
    impl Plugin for OutOfBounds {
        fn manifest(&self) -> Manifest {
            Manifest::new("cv_oob", "0.1.0")
                .bind(Bind::files(["*.pnt"]))
                .shows(ViewKind::Canvas)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            struct S;
            impl Session for S {
                fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
                    Ok(View::Canvas(CanvasView::new(4, 4).image(CanvasImage::new(
                        1,
                        CellRect::new(3, 3, 4, 4),
                        ImageData::Encoded(Arc::from(Vec::<u8>::new())),
                    ))))
                }
            }
            Ok(Box::new(S))
        }
    }
    let dir = sandbox("cv_oob");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let marker = test_plugins::register("cv_oob", Arc::new(OutOfBounds));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
    s.select("a.pnt");
    s.enter();
    assert!(!s.app.is_canvas_preview());
    s.see("outside the canvas");
}

#[test]
fn e2e_canvas_undeclared_canvas_gets_the_existing_undeclared_message() {
    let o = open_with(
        "cv_undeclared",
        Painter::new("cv_undeclared", Draw::Pattern { cols: 4, rows: 2 }).undeclared(),
        90,
        26,
        "en",
    );
    assert!(!o.s.app.is_canvas_preview());
    o.s.see("returned a canvas view but declares only text");
}

// ---- the keys ---------------------------------------------------------------------------------

fn bump() -> PluginAct {
    PluginAct::new("cv_acts", "bump")
}

fn with_acts(name: &str) -> Opened {
    open_with(
        name,
        Painter::new(name, Draw::Pattern { cols: 30, rows: 6 }).with_acts(),
        90,
        26,
        "en",
    )
}

#[test]
fn e2e_canvas_a_declared_action_key_works_and_redraws() {
    let mut o = open_with(
        "cv_acts",
        Painter::new("cv_acts", Draw::Pattern { cols: 30, rows: 6 }).with_acts(),
        90,
        26,
        "en",
    );
    assert_eq!(o.s.app.key_context(), Some(Context::Canvas));
    o.s.see("n=0 one");
    // `j` is a host key in a text view (scroll) but free in a canvas: the plugin's.
    o.s.key('j');
    o.s.see("n=1 one");
    o.s.key('j');
    o.s.see("n=2 one");
    assert_eq!(o.p.acts_run.load(Ordering::SeqCst), 2);
    o.s.key('x');
    o.s.see("*STAR*");
    let _ = bump();
}

#[test]
fn e2e_canvas_the_footer_the_help_and_the_menu_show_the_plugins_actions() {
    let mut o = with_acts("cv_acts_hints");
    let f = footer(&o.s);
    assert!(f.contains("j:bump"), "footer: {f}");
    assert!(f.contains("x:star"), "footer: {f}");
    // The host's own keys that act are in the footer too.
    assert!(f.contains("q:back"), "footer: {f}");
    // ... and only those: nothing that would scroll or search.
    assert!(!f.contains("scroll"), "footer: {f}");
    assert!(!f.contains("search"), "footer: {f}");

    o.s.key('?');
    o.s.see("Add one to the counter");
    o.s.see("Draw a star");
    o.s.see("Preview: plugin view");
    o.s.dont_see("Scroll down");
    o.s.esc();

    // The Space menu offers them under their keys.
    let items: Vec<_> =
        ops::menu::menu(&o.s.app, Context::Canvas, LeaderId::Actions, MenuView::Open)
            .map(|m| {
                m.items
                    .into_iter()
                    .map(|i| (i.key.label(), i.label))
                    .collect()
            })
            .unwrap_or_default();
    assert!(
        items.contains(&("j".to_string(), "bump".to_string())),
        "{items:?}"
    );
    assert!(
        items.contains(&("x".to_string(), "star".to_string())),
        "{items:?}"
    );
    o.s.key(' ');
    assert_eq!(o.s.app.pending_leader, Some(LeaderId::Actions));
    o.s.key('j');
    o.s.see("n=1 one");
}

#[test]
fn e2e_canvas_the_hints_are_in_japanese_when_the_ui_is() {
    let mut o = open_with(
        "cv_acts_ja",
        Painter::new("cv_acts_ja", Draw::Pattern { cols: 30, rows: 6 }).with_acts(),
        90,
        26,
        "ja",
    );
    let tokens = crate::ui::preview::footer_hints(&o.s.app);
    assert!(tokens.contains(&"j:加算".to_string()), "{tokens:?}");
    assert!(tokens.contains(&"x:星".to_string()), "{tokens:?}");
    o.s.key('?');
    // (the drawn text of wide characters has spaces between the cells: look for a part)
    o.s.see("カ");
}

#[test]
fn e2e_canvas_host_motion_keys_do_nothing_without_a_plugin_action() {
    let mut o = open(
        "cv_nomotion",
        Draw::Filled {
            cols: 200,
            rows: 80,
        },
    );
    let before = o.s.screen();
    for c in [
        'j', 'k', 'h', 'l', 'g', 'G', '0', '$', 'n', 'N', 'v', 'V', '/', '+', '-', 'm', 'r',
    ] {
        o.s.key(c);
        assert_eq!(o.s.screen(), before, "`{c}` changed a canvas");
        assert_eq!(o.s.app.pending_leader, None);
        assert!(o.s.app.is_canvas_preview());
        assert_eq!(o.s.app.tab.preview_scroll, 0);
    }
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::Tab,
        KeyCode::Enter,
    ] {
        o.s.press(code, KeyModifiers::NONE);
        assert_eq!(o.s.app.tab.preview_scroll, 0, "{code:?}");
        assert!(o.s.app.is_canvas_preview(), "{code:?}");
    }
    o.s.ctrl('d');
    o.s.ctrl('u');
    o.s.ctrl('n');
    o.s.ctrl('p');
    assert_eq!(o.s.app.tab.preview_scroll, 0);
    assert!(o.s.app.is_canvas_preview());
    assert_eq!(o.s.screen(), before);
    assert!(!o.p.requests().is_empty());
}

#[test]
fn e2e_canvas_a_plugin_may_take_the_keys_the_host_leaves_free() {
    // `j` for one plugin action works (above); `k`, `h`, `g`... are free in the same way, and the
    // keys the host keeps cannot be taken: the registry refuses `q` for a canvas.
    use konoma_plugin::keys::{reserved, KeySpec};
    let r: Vec<KeySpec> = reserved("canvas").into_iter().map(|(k, _)| k).collect();
    let has = |c: char| r.contains(&KeySpec::ch(c));
    for c in ['q', 'e', 'o', 'p', '!', ',', ' ', 'R', 'J', 'K', 'O', '?'] {
        assert!(has(c), "{c} is the host's in a canvas");
    }
    for c in ['j', 'k', 'h', 'l', 'g', 'G', '/', 'n', 'v', 'y', 'm', 'r'] {
        assert!(!has(c), "{c} is free in a canvas");
    }
}

#[test]
fn e2e_canvas_q_and_esc_leave_the_preview() {
    let mut o = open("cv_q", Draw::Pattern { cols: 10, rows: 4 });
    assert_eq!(o.s.app.tab.mode, crate::app::Mode::Preview);
    o.s.key('q');
    assert_eq!(o.s.app.tab.mode, crate::app::Mode::Tree, "q");

    let mut o = open("cv_esc", Draw::Pattern { cols: 10, rows: 4 });
    o.s.esc();
    assert_eq!(o.s.app.tab.mode, crate::app::Mode::Tree, "Esc");
}

#[test]
fn e2e_canvas_the_host_keys_that_stay_still_act() {
    let mut o = open_with(
        "cv_hostkeys",
        Painter::new("cv_hostkeys", Draw::Pattern { cols: 30, rows: 6 }).with_reprs(),
        90,
        26,
        "en",
    );
    // `R` walks the representations the plugin declared: the plugin is asked for `two`.
    assert!(footer(&o.s).contains("R:Two"), "footer: {}", footer(&o.s));
    o.s.see("n=0 one");
    o.s.key('R');
    o.s.see("n=0 two");
    let last = o.p.requests().last().cloned().unwrap();
    assert_eq!(last.req.repr.as_deref(), Some("two"));
    o.s.key('R');
    o.s.see("n=0 one");

    // `,` opens the options menu, `Space` the menu of operations; `Esc` closes them.
    o.s.key(',');
    assert_eq!(o.s.app.pending_leader, Some(LeaderId::Options));
    o.s.key('q');
    assert_eq!(o.s.app.pending_leader, None);
    assert!(o.s.app.is_canvas_preview());
    o.s.key(' ');
    assert_eq!(o.s.app.pending_leader, Some(LeaderId::Actions));
    o.s.key('q');

    // Layer 0 still works: `?` opens the help, a new tab opens with `t`.
    o.s.key('?');
    o.s.see("Preview: plugin view");
    o.s.key('?');
    let tabs = o.s.app.tab_count();
    o.s.key('t');
    assert_eq!(o.s.app.tab_count(), tabs + 1);
}

#[test]
fn e2e_canvas_the_host_binds_only_the_reserved_keys_in_a_canvas() {
    // Every key the default keymap resolves in a canvas is one of `keys::reserved("canvas")`, or
    // a layer 0 key (`global`).
    use konoma_plugin::keys::{reserved, KeySpec};
    let o = open("cv_bound", Draw::Pattern { cols: 10, rows: 4 });
    let allowed: Vec<KeySpec> = reserved("canvas").into_iter().map(|(k, _)| k).collect();
    for (kp, action) in o.s.app.keymaps.bound_actions(Context::Canvas) {
        let crossterm::event::KeyCode::Char(c) = kp.code else {
            continue;
        };
        if kp.ctrl {
            panic!("{kp:?} => {action:?}: no Ctrl key is bound in a canvas");
        }
        assert!(
            allowed.contains(&KeySpec::ch(c)),
            "`{c}` => {action:?} is bound in a canvas but is not reserved for the host"
        );
    }
    for (_, leader) in o.s.app.keymaps.leader_keys_of(Context::Canvas) {
        assert!(
            matches!(leader, LeaderId::Actions | LeaderId::Options),
            "{leader:?}"
        );
    }
}

#[test]
fn e2e_canvas_a_plugin_that_declares_a_reserved_key_does_not_get_it() {
    // The registry drops a declared key the host keeps (here `q`); the action stays reachable
    // from the `Space` menu and the host's `q` still leaves the preview.
    let name = "cv_reserved";
    let dir = sandbox(name);
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let mut p = Painter::new(name, Draw::Pattern { cols: 10, rows: 4 });
    p.acts = true;
    let marker = test_plugins::register(name, Arc::new(Takes(p.clone())));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
    s.select("a.pnt");
    s.enter();
    s.key('q');
    assert_eq!(s.app.tab.mode, crate::app::Mode::Tree);
    assert_eq!(p.acts_run.load(Ordering::SeqCst), 0);

    struct Takes(Painter);
    impl Plugin for Takes {
        fn manifest(&self) -> Manifest {
            Manifest::new(self.0.id.as_str(), "0.1.0")
                .bind(Bind::files(["*.pnt"]))
                .shows(ViewKind::Canvas)
                .action(Act::new("steal").key("q").label(Tr::new("steal")))
        }
        fn open(&self, i: Input, c: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            self.0.open(i, c)
        }
    }
}
