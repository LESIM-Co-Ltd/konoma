//! A complete konoma plugin that draws with ratatui widgets, run without konoma or a terminal.
//!
//! `cargo run -p konoma-plugin-ratatui --example clock`
//!
//! The plugin shows files named `*.konoma-demo` as a canvas: a titled `Block`, a `Gauge` that
//! follows the seconds since the file was opened (`redraw_after` = one second), a small picture,
//! the notes typed so far, and a line editor (`i` enters text input, `Enter` keeps the line,
//! `Esc` leaves). `c` clears the notes. `main` drives it with the helpers of
//! `konoma_plugin::testing` and prints what the canvas looks like.
//!
//! Practices shown here, worth copying:
//!
//! - `view` is a function of the session's state: it can be called on a resize or after a key, so
//!   the time comes from an `Instant` taken at `open`, never from a counter advanced in `view`.
//! - `key` ignores keys held with `Ctrl` or `Alt` (answers `Unhandled`) instead of typing them.
//! - A picture has a stable id: the pixels are sent once and later answers say "unchanged"
//!   (`CanvasImage::unchanged`), so nothing is decoded again.
//! - Colors follow `cx.term.colors`: the host does not convert them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use konoma_plugin::prelude::*;
use konoma_plugin::testing::{open_bytes, send_keys, view_canvas, CanvasText};
use konoma_plugin::view::ImageData;
use konoma_plugin_ratatui::draw_canvas;
use konoma_plugin_ratatui::ratatui::layout::{Constraint, Layout as RLayout};
use konoma_plugin_ratatui::ratatui::style::{
    Color as RColor, Modifier as RModifier, Style as RStyle,
};
use konoma_plugin_ratatui::ratatui::widgets::{Block, Gauge, Paragraph, Widget};

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
        Manifest::new("clock-demo", "0.1.0")
            .name(t!(en: "Clock demo", ja: "時計のデモ"))
            .bind(Bind::files(["*.konoma-demo"]))
            .shows(ViewKind::Canvas)
            .action(
                Act::new("edit")
                    .key("i")
                    .label(t!(en: "edit", ja: "入力"))
                    .help(t!(en: "Type a note", ja: "メモを入力する"))
                    .footer(1),
            )
            .action(
                Act::new("clear")
                    .key("c")
                    .label(t!(en: "clear", ja: "消す"))
                    .help(t!(en: "Forget the notes", ja: "メモを消す")),
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
    typing: bool,
    line: String,
    notes: Vec<String>,
}

impl Session for ClockSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let secs = self.started.elapsed().as_secs();
        let (accent, ok) = palette(cx.term.colors);
        let mut canvas = draw_canvas(req, |buf| {
            let outer = Block::bordered()
                .title(self.title.as_str())
                .border_style(RStyle::new().fg(accent));
            let inner = outer.inner(buf.area);
            outer.render(buf.area, buf);
            let [gauge, body] =
                RLayout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            // Leave the last two cells of the gauge row to the picture.
            let [gauge, _badge] =
                RLayout::horizontal([Constraint::Min(0), Constraint::Length(2)]).areas(gauge);
            Gauge::default()
                .ratio((secs % 10) as f64 / 10.0)
                .label(format!("{secs} s"))
                .gauge_style(RStyle::new().fg(ok).bg(RColor::DarkGray))
                .render(gauge, buf);
            let mut text: Vec<String> = self.notes.iter().map(|n| format!("- {n}")).collect();
            text.push(if self.typing {
                format!("> {}_", self.line)
            } else {
                "press i to type, c to clear".to_string()
            });
            Paragraph::new(text.join("\n"))
                .style(RStyle::new().add_modifier(RModifier::BOLD))
                .render(body, buf);
        });
        // The picture sits in the last two cells of the gauge row, only where it fits inside the
        // area and the terminal can show pixels. Its pixels go over once per area; after that the
        // answer says "unchanged" (the host keeps the picture under the id). A plugin that wants
        // to be sure sends the pixels again, which costs one comparison of the bytes.
        if cx.term.pixels && req.cols >= 8 && req.rows >= 3 {
            let rect = CellRect::new(req.cols - 3, 1, 2, 1);
            let image = if self.badge_sent == Some((req.cols, req.rows)) {
                CanvasImage::unchanged(BADGE, rect)
            } else {
                self.badge_sent = Some((req.cols, req.rows));
                CanvasImage::new(
                    BADGE,
                    rect,
                    ImageData::Rgba {
                        width: BADGE_SIDE,
                        height: BADGE_SIDE,
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

/// The accent and the "ok" color for what the terminal shows: the host does not convert colors.
fn palette(depth: ColorDepth) -> (RColor, RColor) {
    match depth {
        ColorDepth::TrueColor => (RColor::Rgb(80, 200, 220), RColor::Rgb(90, 200, 120)),
        ColorDepth::Ansi256 => (RColor::Indexed(80), RColor::Indexed(77)),
        _ => (RColor::Cyan, RColor::Green),
    }
}

const BADGE_SIDE: u32 = 16;

/// A small RGBA gradient, the picture.
fn badge_pixels() -> Vec<u8> {
    let mut px = Vec::with_capacity((BADGE_SIDE * BADGE_SIDE * 4) as usize);
    for y in 0..BADGE_SIDE {
        for x in 0..BADGE_SIDE {
            px.extend_from_slice(&[(x * 16) as u8, (y * 16) as u8, 160, 255]);
        }
    }
    px
}

// The tests of this crate include this file as a module, where `main` is unused.
#[allow(dead_code)]
fn main() -> Result<(), Failure> {
    let (mut session, _file) = open_bytes(&Clock::default(), "a.konoma-demo", b"Hello, konoma\n")?;
    let req = ViewReq::new(34, 7);
    println!("{}\n", CanvasText::of(&view_canvas(&mut *session, &req)?));

    session.act("edit", &ActCx::bare())?;
    let typed: Vec<Key> = "hi 日本"
        .chars()
        .map(Key::char)
        .chain([Key::new(KeyCode::Enter)])
        .collect();
    send_keys(&mut *session, &typed)?;
    let canvas = view_canvas(&mut *session, &req)?;
    println!("{}", CanvasText::of(&canvas));
    println!(
        "\ntext_input = {}, redraw_after = {:?}",
        canvas.text_input, canvas.redraw_after
    );
    Ok(())
}
