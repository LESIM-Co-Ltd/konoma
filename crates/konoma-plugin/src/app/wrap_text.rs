//! The window of a text that wraps: where it is counted in screen rows
//! ([`konoma_widgets::wrap_view`], shared with the konoma host). A key only records its move;
//! the next view, which holds the lines, makes it. A text that does not wrap never comes here.

use std::collections::HashMap;

use konoma_host_core::keymap::Motion;
use konoma_widgets::scroll::ScrollExtent;
use konoma_widgets::wrap_view::{byte_in_line, drawn, Drawn, Pos, Wrapped};
use ratatui::text::Line;

use crate::host::project::preview_gutter::GutterMark;

/// A move waiting for the lines it counts.
#[derive(Clone, Copy, Debug)]
pub(super) enum TextMove {
    /// A key that moves the caret.
    Key(Motion),
    /// The caret was put on a line (a change block): its whole line is brought on screen.
    Follow,
}

/// What a wrapping window keeps between views besides its first line.
#[derive(Debug, Default)]
pub(super) struct WrapWindow {
    /// The screen rows of the first line already scrolled past.
    pub skip: usize,
    /// The moves the next view makes.
    pub moves: Vec<TextMove>,
    /// The rows, the cells and whether there is a gutter, as last drawn (`None`: not drawn).
    drawn: Option<(usize, u16, bool)>,
}

/// The first line, the caret's line and its column.
pub(super) struct Cursor {
    pub top: usize,
    pub line: usize,
    pub col: usize,
}

/// The size and the dress of the text being drawn.
pub(super) struct Shape<'a> {
    /// The lines of the text, the rows of the view and its cells.
    pub total: usize,
    pub rows: usize,
    pub width: u16,
    pub tab_width: usize,
    pub marks: Option<&'a HashMap<u32, GutterMark>>,
}

/// Where the window ended up, with what the scroll extent needs.
pub(super) struct Placed {
    pos: Pos,
    pos_rows: usize,
    end: Pos,
    end_rows: usize,
}

impl Placed {
    /// The extent in bytes of a text `len` bytes long; `byte_of` answers where a line starts.
    pub fn extent(&self, len: u64, mut byte_of: impl FnMut(usize) -> u64) -> ScrollExtent {
        let mut at = |p: Pos, rows: usize| {
            let start = byte_of(p.line);
            if p.skip == 0 {
                start
            } else {
                byte_in_line(start, byte_of(p.line + 1), p.skip, rows)
            }
        };
        let pos = at(self.pos, self.pos_rows);
        let end = at(self.end, self.end_rows);
        ScrollExtent::new(pos, end, len.saturating_sub(end))
    }

    /// The first line of the window.
    pub fn top(&self) -> usize {
        self.pos.line
    }
}

impl WrapWindow {
    /// The window starts at the first row of its line again (it was put there, or the wrap
    /// changed and every line takes other rows).
    pub fn restart(&mut self) {
        self.skip = 0;
        self.drawn = None;
    }

    /// Makes the waiting moves over the lines `raw` gives and keeps the window inside the text.
    pub fn settle(
        &mut self,
        cursor: &mut Cursor,
        shape: &Shape,
        raw: &mut dyn FnMut(usize) -> Line<'static>,
    ) -> Placed {
        let &Shape {
            total,
            rows,
            width,
            tab_width,
            marks,
        } = shape;
        let mut source = |_: usize, i: usize| -> Drawn {
            let body = konoma_widgets::text::expand_tabs(vec![raw(i)], tab_width)
                .pop()
                .unwrap_or_default();
            drawn(body, |body| {
                super::document::lay_out_lines(vec![body], i, marks)
                    .pop()
                    .unwrap_or_default()
            })
        };
        let mut w = Wrapped::new(total, rows, width, &mut source);
        let height = rows.max(1);
        let last = total.saturating_sub(1);
        let mut pos = Pos {
            line: cursor.top.min(last),
            skip: self.skip,
        };
        cursor.line = cursor.line.min(last);
        let now = (rows, width, marks.is_some_and(|m| !m.is_empty()));
        // The lines take other rows than before: a caret among the window's lines stays on
        // screen.
        if self.drawn != Some(now) {
            let untouched = cursor.line == pos.line && pos.skip == 0 && cursor.col == 0;
            if !untouched && cursor.line >= pos.line && cursor.line < pos.line + height {
                pos = w.follow_line(pos, cursor.line, cursor.col);
            }
            self.drawn = Some(now);
        }
        for m in std::mem::take(&mut self.moves) {
            pos = apply(&mut w, pos, cursor, m, total, height);
        }
        pos = w.clamp(pos);
        cursor.top = pos.line;
        self.skip = pos.skip;
        let end = w.bottom();
        Placed {
            pos,
            pos_rows: w.rows(pos.line, pos.line),
            end,
            end_rows: w.rows(end.line, end.line),
        }
    }
}

/// One move, as the konoma host makes it.
fn apply(
    w: &mut Wrapped,
    pos: Pos,
    cursor: &mut Cursor,
    m: TextMove,
    total: usize,
    height: usize,
) -> Pos {
    let page = i32::try_from(height.saturating_sub(1).max(1)).unwrap_or(i32::MAX);
    let half = i32::try_from((height / 2).max(1)).unwrap_or(i32::MAX);
    let mut c = konoma_widgets::caret::Caret {
        top: pos.line,
        line: cursor.line,
        col: cursor.col,
    };
    let TextMove::Key(m) = m else {
        return w.follow_line(pos, cursor.line, cursor.col);
    };
    let mut paged = |delta: i32| {
        let (next, line, col) = w.page(pos, cursor.line, cursor.col, delta);
        cursor.line = line;
        cursor.col = col;
        next
    };
    match m {
        Motion::PageDown => paged(page),
        Motion::PageUp => paged(-page),
        Motion::HalfDown => paged(half),
        Motion::HalfUp => paged(-half),
        Motion::Down | Motion::Up => {
            c.step(if m == Motion::Down { 1 } else { -1 }, total, height);
            cursor.line = c.line;
            w.follow_line(pos, cursor.line, cursor.col)
        }
        Motion::Top => {
            let (next, col) = w.to_top(cursor.col);
            cursor.line = 0;
            cursor.col = col;
            next
        }
        Motion::Bottom => {
            let (next, line, col) = w.to_bottom(cursor.col);
            cursor.line = line;
            cursor.col = col;
            next
        }
        Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd => {
            match m {
                Motion::Left => c.col_move(-1),
                Motion::Right => c.col_move(1),
                Motion::LineHome => c.col_home(),
                _ => c.col_end(),
            }
            cursor.col = c.col;
            w.follow_row(pos, cursor.line, cursor.col)
        }
    }
}
