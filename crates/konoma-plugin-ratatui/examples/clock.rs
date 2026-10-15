//! A complete konoma plugin that draws with ratatui widgets, run without konoma or a terminal.
//!
//! `cargo run -p konoma-plugin-ratatui --example clock`
//!
//! The plugin shows files named `*.konoma-demo` as a canvas: a titled `Block`, a `Gauge` that
//! moves each time konoma asks for the view again (`redraw_after` = one second), the notes typed
//! so far, and a line editor (`i` enters text input, `Enter` keeps the line, `Esc` leaves). `c`
//! clears the notes. `main` drives it with the helpers of `konoma_plugin::testing` and prints what
//! the canvas looks like.

use std::time::Duration;

use konoma_plugin::key::{Key, KeyCode};
use konoma_plugin::prelude::*;
use konoma_plugin::testing::{open_bytes, send_keys, view_canvas, CanvasText};
use konoma_plugin_ratatui::draw_canvas;
use ratatui::layout::{Constraint, Layout as RLayout};
use ratatui::style::{Color as RColor, Modifier as RModifier, Style as RStyle};
use ratatui::widgets::{Block, Gauge, Paragraph, Widget};

pub struct Clock;

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
            frames: 0,
            typing: false,
            line: String::new(),
            notes: Vec::new(),
        }))
    }
}

struct ClockSession {
    title: String,
    /// How many times the view was asked for: it moves the gauge.
    frames: u64,
    typing: bool,
    line: String,
    notes: Vec<String>,
}

impl Session for ClockSession {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.frames += 1;
        let canvas = draw_canvas(req, |buf| {
            let outer = Block::bordered()
                .title(self.title.as_str())
                .border_style(RStyle::new().fg(RColor::Cyan));
            let inner = outer.inner(buf.area);
            outer.render(buf.area, buf);
            let [gauge, body] =
                RLayout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            Gauge::default()
                .ratio((self.frames % 10) as f64 / 10.0)
                .label(format!("frame {}", self.frames))
                .gauge_style(RStyle::new().fg(RColor::Green).bg(RColor::DarkGray))
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

#[allow(dead_code)]
fn main() -> Result<(), Failure> {
    let (mut session, _file) = open_bytes(&Clock, "a.konoma-demo", b"Hello, konoma\n")?;
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
