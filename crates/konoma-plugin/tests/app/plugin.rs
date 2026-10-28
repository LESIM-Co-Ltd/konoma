//! A small plugin for the app tests. What a file shows follows its extension:
//!
//! - `.t`: a text view over the file itself (read in windows)
//! - `.o`: the file as an owned string
//! - `.l`: styled lines (line 0 is red)
//! - `.c`: a canvas of `x` as big as the request, with the first row's first cell `A` in blue;
//!   `.big` is a fixed 40x12 canvas whatever the request
//! - `.hostile`: `hostile_canvas()`, cells that fail `CanvasView::check`
//! - `.m`: a message
//! - `.list`: a list (a display type the app cannot show yet)
//! - `.fail`: the view fails
//! - `.noopen`: the open fails

use std::time::Duration;

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListView, MessageView, TextSource, TextView};

pub struct TestPlugin {
    pub id: &'static str,
    pub globs: &'static [&'static str],
    /// Added to a canvas's `redraw_after` (milliseconds); 0 = none.
    pub redraw_ms: u64,
}

impl TestPlugin {
    pub fn all() -> TestPlugin {
        TestPlugin {
            id: "app-test",
            globs: &[
                "*.t",
                "*.o",
                "*.l",
                "*.c",
                "*.big",
                "*.hostile",
                "*.m",
                "*.list",
                "*.fail",
                "*.noopen",
            ],
            redraw_ms: 0,
        }
    }
}

impl Plugin for TestPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id, "0.0.1")
            .bind(Bind::files(self.globs.iter().copied()))
            .shows(ViewKind::Text)
            .shows(ViewKind::Canvas)
            .shows(ViewKind::List)
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(t!(en: "a file only")));
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if ext == "noopen" {
            return Err(Failure::corrupt(
                t!(en: "the test plugin refuses this file"),
            ));
        }
        Ok(Box::new(TestSession {
            path,
            ext,
            redraw_ms: self.redraw_ms,
            id: self.id,
        }))
    }
}

struct TestSession {
    path: std::path::PathBuf,
    ext: String,
    redraw_ms: u64,
    id: &'static str,
}

impl Session for TestSession {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Ok(match self.ext.as_str() {
            "t" => View::Text(TextView {
                source: TextSource::File(self.path.clone()),
                styler: None,
                file_lines: true,
                syntax_name: None,
            }),
            "o" => View::Text(TextView {
                source: TextSource::Owned(std::fs::read_to_string(&self.path)?),
                styler: None,
                file_lines: true,
                syntax_name: None,
            }),
            "l" => {
                let text = std::fs::read_to_string(&self.path)?;
                let lines = text
                    .lines()
                    .enumerate()
                    .map(|(i, l)| {
                        if i == 0 {
                            Line::from(Span::styled(l.to_string(), Style::new().fg(Color::Red)))
                        } else {
                            Line::raw(l)
                        }
                    })
                    .collect();
                View::Text(TextView {
                    source: TextSource::Lines(lines),
                    styler: None,
                    file_lines: false,
                    syntax_name: None,
                })
            }
            "c" | "big" => {
                let (cols, rows) = if self.ext == "big" {
                    (40, 12)
                } else {
                    (req.cols, req.rows)
                };
                let mut c = CanvasView::new(cols, rows);
                c.fill(CellRect::new(0, 0, cols, rows), Style::new());
                for y in 0..rows {
                    c.set_string(0, y, &"x".repeat(usize::from(cols)), Style::new());
                }
                c.set_string(0, 0, "A", Style::new().fg(Color::Blue).bold());
                if self.redraw_ms > 0 {
                    c.redraw_after = Some(Duration::from_millis(self.redraw_ms));
                }
                View::Canvas(c)
            }
            "hostile" => View::Canvas(hostile_canvas()),
            "m" => View::Message(MessageView {
                lines: vec![Line::raw(format!("hello from {}", self.id))],
            }),
            "list" => View::List(ListView { rows: Vec::new() }),
            "fail" => return Err(Failure::corrupt(t!(en: "broken on purpose"))),
            _ => return Err(Failure::unsupported(t!(en: "unknown"))),
        })
    }
}

/// An 8x2 canvas of what the host must not copy as it is (none of it passes `CanvasView::check`
/// but an in-process plugin is not checked): a control character, the kitty placeholder, a wide
/// character at the last visible column, a continuation cell with a style of its own.
pub fn hostile_canvas() -> CanvasView {
    let mut c = CanvasView::new(8, 2);
    let row0 = ["a", "\u{1b}", "日", "", "\u{10EEEE}", "日", "", "z"];
    let row1 = ["\u{7}", "b", "\n", "日", "", "c", "d", "e"];
    for (i, s) in row0.iter().chain(row1.iter()).enumerate() {
        c.cells[i].symbol = s.to_string();
    }
    c.cells[3].style = Style::new().bg(Color::Red);
    c.cells[4].style = Style::new().fg(Color::Green);
    c.cells[11].style = Style::new().bg(Color::Red);
    c
}
