//! Draw a konoma plugin's canvas with ratatui widgets.
//!
//! A plugin that returns a [`konoma_plugin::CanvasView`] draws its own cells. This crate lets it do
//! that with ratatui: inside `Session::view` (which konoma calls on a worker, never on its UI
//! thread) the plugin renders widgets into a [`ratatui::buffer::Buffer`] it owns, then converts
//! the buffer.
//!
//! ```
//! use konoma_plugin::prelude::*;
//! use konoma_plugin_ratatui::ratatui::widgets::{Block, Widget};
//!
//! struct Hello;
//! impl Session for Hello {
//!     fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
//!         let canvas = konoma_plugin_ratatui::draw_canvas(req, |buf| {
//!             Block::bordered().title("hello").render(buf.area, buf);
//!         });
//!         Ok(View::Canvas(canvas))
//!     }
//! }
//!
//! let view = Hello.view(&ViewReq::new(20, 3), &ViewCx::bare()).unwrap();
//! let View::Canvas(canvas) = view else { panic!("a canvas") };
//! assert!(canvas.check().is_ok());
//! assert_eq!(canvas.cells[1].symbol, "h");
//! ```
//!
//! **Which ratatui.** The [`ratatui::buffer::Buffer`] you draw into belongs to this crate, so use
//! the ratatui it re-exports (`konoma_plugin_ratatui::ratatui`) or depend on the same ratatui
//! version as this crate (0.30.x): a plugin built on another ratatui gets a confusing "expected
//! `Buffer`, found `Buffer`" type error. What crosses into konoma is plain data, so konoma's own
//! ratatui version does not matter, and each ratatui minor release needs a new version of this
//! crate. There is no `Frame` here (no `render_widget`, no cursor position): render with
//! `Widget::render(area, buf)` or `StatefulWidget::render(area, buf, &mut state)`.
//!
//! Pictures are not part of a `Buffer`. A plugin that wants them adds [`CanvasImage`]s to the
//! returned canvas itself (`canvas.image(..)`), and sets `redraw_after` / `text_input` the same
//! way.
//!
//! [`CanvasImage`]: konoma_plugin::CanvasImage

use konoma_plugin::style::{Color, Modifier, Style};
use konoma_plugin::view::{CanvasCell, CanvasView, ViewReq};
/// The ratatui this crate is built with. A plugin should draw with this one (or depend on the same
/// version): the `Buffer` handed to [`draw_canvas`] is this ratatui's.
pub use ratatui;
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier as RModifier};

/// A blank buffer the size of the request's area (`cols` x `rows` cells, at the origin).
pub fn canvas_buffer(req: &ViewReq) -> Buffer {
    Buffer::empty(Rect::new(0, 0, req.cols, req.rows))
}

/// Draw into a buffer sized to the request and convert it: the whole round in one call.
pub fn draw_canvas(req: &ViewReq, f: impl FnOnce(&mut Buffer)) -> CanvasView {
    let mut buf = canvas_buffer(req);
    f(&mut buf);
    canvas_from_buffer(&buf)
}

/// Convert a ratatui buffer into a canvas of the same size (`buf.area`'s width and height).
///
/// * A wide character (display width 2 or more) keeps its symbol in its own cell and the cell
///   after it becomes the empty *continuation*, whatever ratatui holds there (a space, an empty
///   string, a skipped cell). A wide character that would not fit whole in its row becomes a
///   blank, so the canvas never holds half a wide character.
/// * A cell with `skip` set, a cell with an empty or zero-width symbol that is not a wide
///   character's continuation, and a symbol a canvas cell may not hold (a control character, the kitty
///   image placeholder U+10EEEE, more than one grapheme, over 32 bytes, wider than two columns) become the default cell (a space).
/// * `Color::Reset` becomes "no color" (`None`), so the host's own default shows through.
///   Underline colors are dropped.
pub fn canvas_from_buffer(buf: &Buffer) -> CanvasView {
    let cols = buf.area.width;
    let rows = buf.area.height;
    let mut canvas = CanvasView::new(cols, rows);
    for row in 0..rows {
        let mut x = 0u16;
        while x < cols {
            let cell = &buf[(buf.area.x + x, buf.area.y + row)];
            let at = row as usize * cols as usize + x as usize;
            if skipped(cell) {
                x += 1;
                continue;
            }
            let sym = cell.symbol();
            // A control character or the image placeholder (U+10EEEE) is not a symbol a canvas
            // cell may hold.
            if sym
                .chars()
                .any(|c| c.is_control() || c == konoma_plugin::view::KITTY_PLACEHOLDER)
            {
                x += 1;
                continue;
            }
            // The one width function of the canvas; a `ForcedWidth` option is ignored because the
            // host draws the symbol, not the option.
            let width = konoma_plugin::symbol_width(sym);
            if width == 0 {
                // Empty or zero-width: nothing to show; a lone one is a blank.
                canvas.cells[at].style = style_of(cell);
                x += 1;
                continue;
            }
            if !CanvasCell::is_plain_symbol(sym) {
                // Several graphemes, a long cluster or a symbol wider than two columns: what a
                // canvas cell may not hold becomes the default cell.
                x += 1;
                continue;
            }
            let style = style_of(cell);
            if width >= 2 {
                if x + 1 < cols {
                    canvas.cells[at] = CanvasCell::new(sym, style);
                    canvas.cells[at + 1] = CanvasCell::new("", style);
                    x += 2;
                } else {
                    // Half a wide character: blank it.
                    canvas.cells[at].style = style;
                    x += 1;
                }
                continue;
            }
            canvas.cells[at] = CanvasCell::new(sym, style);
            x += 1;
        }
    }
    canvas
}

/// Whether the cell is marked to be skipped (the current option, or the field older ratatui
/// versions used).
#[allow(deprecated)]
fn skipped(cell: &Cell) -> bool {
    cell.skip || cell.diff_option == CellDiffOption::Skip
}

/// The canvas style of a buffer cell.
fn style_of(cell: &Cell) -> Style {
    let mut s = Style::new();
    s.fg = color_of(cell.fg);
    s.bg = color_of(cell.bg);
    s.add = modifier_of(cell.modifier);
    s
}

/// A ratatui color as a plugin color. `Reset` is `None`: "no color set".
pub fn color_of(c: RColor) -> Option<Color> {
    Some(match c {
        RColor::Reset => return None,
        RColor::Black => Color::Black,
        RColor::Red => Color::Red,
        RColor::Green => Color::Green,
        RColor::Yellow => Color::Yellow,
        RColor::Blue => Color::Blue,
        RColor::Magenta => Color::Magenta,
        RColor::Cyan => Color::Cyan,
        RColor::Gray => Color::Gray,
        RColor::DarkGray => Color::DarkGray,
        RColor::LightRed => Color::LightRed,
        RColor::LightGreen => Color::LightGreen,
        RColor::LightYellow => Color::LightYellow,
        RColor::LightBlue => Color::LightBlue,
        RColor::LightMagenta => Color::LightMagenta,
        RColor::LightCyan => Color::LightCyan,
        RColor::White => Color::White,
        RColor::Indexed(i) => Color::Indexed(i),
        RColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    })
}

/// A ratatui modifier as a plugin modifier (the bits are at the same positions; a bit the plugin
/// API does not know is dropped).
pub fn modifier_of(m: RModifier) -> Modifier {
    const PAIRS: [(RModifier, Modifier); 9] = [
        (RModifier::BOLD, Modifier::BOLD),
        (RModifier::DIM, Modifier::DIM),
        (RModifier::ITALIC, Modifier::ITALIC),
        (RModifier::UNDERLINED, Modifier::UNDERLINED),
        (RModifier::SLOW_BLINK, Modifier::SLOW_BLINK),
        (RModifier::RAPID_BLINK, Modifier::RAPID_BLINK),
        (RModifier::REVERSED, Modifier::REVERSED),
        (RModifier::HIDDEN, Modifier::HIDDEN),
        (RModifier::CROSSED_OUT, Modifier::CROSSED_OUT),
    ];
    let mut out = Modifier::EMPTY;
    for (r, p) in PAIRS {
        if m.contains(r) {
            out |= p;
        }
    }
    out
}
