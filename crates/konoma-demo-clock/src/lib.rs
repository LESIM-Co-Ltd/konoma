//! A complete konoma plugin that draws with ratatui widgets (the "clock" demo).
//!
//! It runs as a program of its own (`src/main.rs`: `konoma_plugin::export!`), which konoma loads
//! with `konoma --plugin <path>`; `samples/demo.konoma-demo` is a file to open with it.
//!
//! The plugin shows files named `*.konoma-demo` as a canvas: a titled `Block`, a `Gauge` that
//! follows the seconds since the file was opened (`redraw_after` = one second), a small picture
//! (where the terminal can show pixels), the notes typed so far, and a line editor (`i` enters
//! text input, `Enter` keeps the line, `Esc` leaves). `c` clears the notes; `Left`/`Right` change
//! the gauge colour (an action on a fixed key).
//!
//! Practices shown here, worth copying:
//!
//! - `view` is a function of the session's state: it can be called on a resize or after a key, so
//!   the time comes from an `Instant` taken at `open`, never from a counter advanced in `view`.
//! - `key` ignores keys held with `Ctrl` or `Alt` (answers `Unhandled`) instead of typing them.
//! - A picture has a stable id: the pixels are sent once and later answers say "unchanged"
//!   (`CanvasImage::unchanged`), so nothing is decoded again.
//! - Colors follow `cx.term.colors`: the host does not convert them.
//! - Everything is sized to `req.cols` x `req.rows`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use konoma_plugin::prelude::*;
use konoma_plugin::view::ImageData;

use konoma_plugin_ratatui::draw_canvas;
use konoma_plugin_ratatui::ratatui::layout::{Constraint, Layout as RLayout};
use konoma_plugin_ratatui::ratatui::style::{
    Color as RColor, Modifier as RModifier, Style as RStyle,
};
use konoma_plugin_ratatui::ratatui::text::{Line as RLine, Span as RSpan};
use konoma_plugin_ratatui::ratatui::widgets::{Block, Gauge, Paragraph, Widget};

/// The id the plugin registers under.
pub const ID: &str = "demo-clock";

/// The id of the picture: one stable name, whatever the pixels or the place.
const BADGE: u64 = 1;

/// The plugin. `age` is how long the file counts as already open (zero in konoma; a test sets it
/// to move the clock without sleeping).
#[derive(Default)]
pub struct Clock {
    pub age: Duration,
}

impl Plugin for Clock {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "0.1.0")
            .name(t!(en: "Clock demo", ja: "時計のデモ"))
            .bind(Bind::files(["*.konoma-demo"]))
            .shows(ViewKind::Canvas)
            .action(
                Act::new("edit")
                    .key("i")
                    .label(t!(en: "note", ja: "メモ"))
                    .help(t!(en: "Type a note", ja: "メモを入力"))
                    .footer(1),
            )
            .action(
                Act::new("colour_next")
                    .key("Right")
                    .label(t!(en: "colour", ja: "色"))
                    .help(t!(en: "Next gauge colour", ja: "ゲージの色を次へ"))
                    .footer(2),
            )
            .action(
                Act::new("colour_prev")
                    .key("Left")
                    .label(t!(en: "colour back", ja: "色を戻す"))
                    .help(t!(en: "Previous gauge colour", ja: "ゲージの色を前へ")),
            )
            .action(
                Act::new("clear")
                    .key("c")
                    .label(t!(en: "clear notes", ja: "メモを消す"))
                    .help(t!(en: "Forget all notes", ja: "メモをすべて消す")),
            )
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let title = match &input {
            Input::File { path, .. } => std::fs::read_to_string(path)?
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
            _ => String::new(),
        };
        Ok(Box::new(ClockSession {
            title: if title.is_empty() {
                "Clock".to_string()
            } else {
                title
            },
            started: Instant::now()
                .checked_sub(self.age)
                .unwrap_or_else(Instant::now),
            badge: Arc::new(badge_pixels()),
            badge_sent: None,
            hue: 0,
            typing: false,
            line: String::new(),
            notes: Vec::new(),
        }))
    }
}

struct ClockSession {
    title: String,
    /// When the file was opened: the gauge follows the time since, not the number of views.
    started: Instant,
    /// The pixels of the picture, made once and shared with every answer.
    badge: Arc<Vec<u8>>,
    /// The area the host last got the pixels for (it holds them under `BADGE` until the picture
    /// is dropped), so later answers say "unchanged".
    badge_sent: Option<(u16, u16)>,
    /// Which gauge colour is on (an index into [`HUES`]).
    hue: usize,
    typing: bool,
    line: String,
    notes: Vec<String>,
}

impl ClockSession {
    /// The notes (newest kept when the area is short), then, at the bottom, either the hint line
    /// or - while typing - a banner and a highlighted prompt with a block cursor.
    fn draw_text(
        &self,
        body: konoma_plugin_ratatui::ratatui::layout::Rect,
        buf: &mut konoma_plugin_ratatui::ratatui::buffer::Buffer,
        lang: &str,
        accent: RColor,
    ) {
        let bottom = if self.typing { 2 } else { 1 }.min(body.height);
        let [notes_area, bottom_area] =
            RLayout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(body);
        let rows = notes_area.height as usize;
        if rows > 0 && !self.notes.is_empty() {
            let heading = rows >= 2;
            let room = rows - usize::from(heading);
            let shown = &self.notes[self.notes.len().saturating_sub(room)..];
            let mut lines: Vec<RLine> = Vec::new();
            if heading {
                lines.push(RLine::styled(
                    t!(en: "Notes", ja: "メモ").get(lang).to_string(),
                    RStyle::new().fg(accent).add_modifier(RModifier::BOLD),
                ));
            }
            lines.extend(shown.iter().map(|n| RLine::from(format!("- {n}"))));
            Paragraph::new(lines).render(notes_area, buf);
        }
        if bottom_area.height == 0 {
            return;
        }
        if self.typing {
            let rect = |y| {
                konoma_plugin_ratatui::ratatui::layout::Rect::new(
                    bottom_area.x,
                    y,
                    bottom_area.width,
                    1,
                )
            };
            let prompt_y = bottom_area.y + bottom_area.height - 1;
            if bottom_area.height >= 2 {
                let banner = t!(
                    en: "Typing - type text, Enter adds it as a note, Esc leaves",
                    ja: "入力中 — 文字を打って Enter で追加 / Esc で抜ける"
                );
                Paragraph::new(banner.get(lang).to_string())
                    .style(RStyle::new().fg(accent).add_modifier(RModifier::BOLD))
                    .render(rect(bottom_area.y), buf);
            }
            let prompt = RLine::from(vec![
                RSpan::raw(format!("> {}", self.line)),
                RSpan::styled("█", RStyle::new().fg(accent)),
            ]);
            Paragraph::new(prompt)
                .style(RStyle::new().fg(RColor::White).bg(RColor::DarkGray))
                .render(rect(prompt_y), buf);
        } else {
            let hint = if self.notes.is_empty() {
                t!(
                    en: "i: type a note   \u{2190}/\u{2192}: gauge colour",
                    ja: "i: メモを入力   \u{2190}/\u{2192}: ゲージの色"
                )
            } else {
                t!(
                    en: "i: type a note   \u{2190}/\u{2192}: gauge colour   c: clear notes",
                    ja: "i: メモを入力   \u{2190}/\u{2192}: ゲージの色   c: メモを消す"
                )
            };
            Paragraph::new(hint.get(lang).to_string())
                .style(RStyle::new().add_modifier(RModifier::DIM))
                .render(bottom_area, buf);
        }
    }
}

impl Session for ClockSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let secs = self.started.elapsed().as_secs();
        let (accent, ok) = palette(cx.term.colors, self.hue);
        let show_badge = cx.term.pixels && req.cols >= BADGE_MIN_COLS && req.rows >= BADGE_MIN_ROWS;
        let mut canvas = draw_canvas(req, |buf| {
            let outer = Block::bordered()
                .title(self.title.as_str())
                .border_style(RStyle::new().fg(accent));
            let inner = outer.inner(buf.area);
            outer.render(buf.area, buf);
            let [gauge, body] =
                RLayout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            Gauge::default()
                .ratio((secs % 10) as f64 / 10.0)
                .label(format!("{secs} s · {}", HUES[self.hue % HUES.len()].name))
                .gauge_style(RStyle::new().fg(ok).bg(RColor::DarkGray))
                .render(gauge, buf);
            // The picture takes the right end of the body; the text keeps to the rest.
            let body = if show_badge {
                RLayout::horizontal([Constraint::Min(0), Constraint::Length(BADGE_CELLS_W + 1)])
                    .areas::<2>(body)[0]
            } else {
                body
            };
            self.draw_text(body, buf, cx.lang, accent);
        });
        // The picture sits at the right end of the body (below the gauge), only when the terminal
        // can show pixels and it fits inside the area. Its pixels go over once per area; after that
        // the answer says "unchanged" (the host keeps the picture under the id). A plugin that
        // wants to be sure sends the pixels again, which costs one comparison of the bytes.
        if show_badge {
            let rect = CellRect::new(
                req.cols - 1 - BADGE_CELLS_W,
                2,
                BADGE_CELLS_W,
                BADGE_CELLS_H,
            );
            let image = if self.badge_sent == Some((req.cols, req.rows)) {
                CanvasImage::unchanged(BADGE, rect)
            } else {
                self.badge_sent = Some((req.cols, req.rows));
                CanvasImage::new(
                    BADGE,
                    rect,
                    ImageData::Rgba {
                        width: BADGE_W,
                        height: BADGE_H,
                        pixels: self.badge.clone(),
                    },
                )
            };
            canvas = canvas.image(image);
        }
        Ok(View::Canvas(
            canvas
                .redraw_after(Duration::from_secs(1))
                .text_input(self.typing),
        ))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        match action {
            "edit" => {
                self.typing = true;
                Ok(Outcome::Redraw)
            }
            "colour_next" => {
                self.hue = (self.hue + 1) % HUES.len();
                Ok(Outcome::Redraw)
            }
            "colour_prev" => {
                self.hue = (self.hue + HUES.len() - 1) % HUES.len();
                Ok(Outcome::Redraw)
            }
            "clear" => {
                self.notes.clear();
                Ok(Outcome::Redraw)
            }
            _ => Ok(Outcome::Unhandled),
        }
    }

    fn key(&mut self, key: &Key, _cx: &ActCx) -> Result<Outcome, Failure> {
        // `Ctrl-x` and `Alt-x` are commands, not text: leave them alone (`Esc` has no modifier).
        if key.mods.contains(KeyMods::CTRL) || key.mods.contains(KeyMods::ALT) {
            return Ok(Outcome::Unhandled);
        }
        match &key.code {
            KeyCode::Char(c) => self.line.push(*c),
            KeyCode::Paste(s) => self.line.push_str(s),
            KeyCode::Backspace => {
                self.line.pop();
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.line);
                if !line.is_empty() {
                    self.notes.push(line);
                }
            }
            // The host leaves the input state by itself; this is the notice.
            KeyCode::Esc => self.typing = false,
            _ => return Ok(Outcome::Unhandled),
        }
        Ok(Outcome::Redraw)
    }

    fn enabled(&self, action: &str) -> bool {
        // Clearing is possible only while there is something to clear.
        action != "clear" || !self.notes.is_empty()
    }
}

/// One gauge colour `Left` / `Right` can pick: the name shown, and the colour for each depth.
struct Hue {
    name: &'static str,
    rgb: (u8, u8, u8),
    indexed: u8,
    named: RColor,
}

const HUES: [Hue; 4] = [
    Hue {
        name: "green",
        rgb: (90, 200, 120),
        indexed: 77,
        named: RColor::Green,
    },
    Hue {
        name: "magenta",
        rgb: (210, 100, 200),
        indexed: 170,
        named: RColor::Magenta,
    },
    Hue {
        name: "yellow",
        rgb: (230, 190, 60),
        indexed: 178,
        named: RColor::Yellow,
    },
    Hue {
        name: "blue",
        rgb: (90, 140, 240),
        indexed: 69,
        named: RColor::Blue,
    },
];

/// The accent and the gauge color for what the terminal shows: the host does not convert colors.
fn palette(depth: ColorDepth, hue: usize) -> (RColor, RColor) {
    let h = &HUES[hue % HUES.len()];
    let (r, g, b) = h.rgb;
    match depth {
        ColorDepth::TrueColor => (RColor::Rgb(80, 200, 220), RColor::Rgb(r, g, b)),
        ColorDepth::Ansi256 => (RColor::Indexed(80), RColor::Indexed(h.indexed)),
        _ => (RColor::Cyan, h.named),
    }
}

/// The picture: 64 x 32 pixels, shown in 8 x 2 cells (cells are about 8 x 16 pixels).
const BADGE_W: u32 = 64;
const BADGE_H: u32 = 32;
const BADGE_CELLS_W: u16 = 8;
const BADGE_CELLS_H: u16 = 2;
/// The smallest area that has room for the picture next to the text.
const BADGE_MIN_COLS: u16 = 24;
const BADGE_MIN_ROWS: u16 = 5;

/// An RGBA gradient with a white border and a dark inner frame, the picture.
fn badge_pixels() -> Vec<u8> {
    let mut px = Vec::with_capacity((BADGE_W * BADGE_H * 4) as usize);
    for y in 0..BADGE_H {
        for x in 0..BADGE_W {
            let edge = x == 0 || y == 0 || x == BADGE_W - 1 || y == BADGE_H - 1;
            let frame = x == 3 || y == 3 || x == BADGE_W - 4 || y == BADGE_H - 4;
            let rgba = if edge {
                [255, 255, 255, 255]
            } else if frame {
                [20, 24, 40, 255]
            } else {
                [
                    (x * 255 / (BADGE_W - 1)) as u8,
                    (y * 255 / (BADGE_H - 1)) as u8,
                    160,
                    255,
                ]
            };
            px.extend_from_slice(&rgba);
        }
    }
    px
}
