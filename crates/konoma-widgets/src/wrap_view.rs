//! The window of a text that wraps: where it is in **screen rows**, not only in file lines.
//!
//! A wrapped line takes several screen rows, so a window is `(first line, rows of that line
//! already scrolled past)`, and the caret follow, the paging, `g` / `G` and the stop at the end
//! all count screen rows. The rows are counted with ratatui's own word wrap over the line as it
//! is drawn (tabs expanded, gutter included), so they are the rows on screen. Only the lines
//! around the window are ever asked for. A text that does not wrap needs none of this:
//! [`crate::caret::Caret`] is its arithmetic.
//!
//! The konoma host and the standalone app both use it.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use unicode_segmentation::UnicodeSegmentation;

use crate::text::wrapped_row_count;
use crate::window::FileWindow;

/// The most rows of one line that can be scrolled past (`Paragraph::scroll` takes a `u16`).
const MAX_LINE_ROWS: usize = u16::MAX as usize;

/// Lines of at least this many bytes have their row count remembered: counting a very long line
/// allocates far more than the line, and a window asks for the same one on every frame.
const REMEMBERED_FROM: usize = 1024;
const REMEMBERED_MOST: usize = 256;

/// The most clusters of a line drawn off screen at once to find where its rows start. A longer
/// line is drawn piece by piece, and its answer is remembered.
const PIECE: usize = 16 * 1024;
const REMEMBERED_STARTS: usize = 4;
/// A piece grows to a few times this (or [`PIECE`]) looking for the start of a word.
const WORD_REACH: usize = 64;

/// (hash of the text, width) and the row starts of that line, the most recently asked last.
type Starts = Vec<((u64, u16), Rc<Vec<Option<usize>>>)>;

thread_local! {
    static REMEMBERED: RefCell<HashMap<(u64, u16), usize>> = RefCell::new(HashMap::new());
    static STARTS: RefCell<Starts> = const { RefCell::new(Vec::new()) };
}

/// `line` with no grapheme cluster split between two spans. ratatui finds clusters span by span,
/// so a caret, a selection or a match that cuts a span inside one (a variation selector, a joined
/// emoji, a flag) would draw the line over other rows than the plain one. The cluster takes the
/// style of the part that stands out (a background or a reversed cell), else of its first part.
/// Both the counting and the drawing of a wrapping text go through this.
pub fn whole_clusters(line: Line<'static>) -> Line<'static> {
    if line.spans.len() < 2 || line.spans.iter().all(|s| s.content.is_ascii()) {
        return line;
    }
    let mut text = String::new();
    let mut ends = Vec::with_capacity(line.spans.len());
    for span in &line.spans {
        text.push_str(&span.content);
        ends.push(text.len());
    }
    let marked = |s: &Style| s.bg.is_some() || s.add_modifier.contains(Modifier::REVERSED);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut piece = String::new();
    let mut piece_style = Style::default();
    let mut part = 0usize;
    for (at, cluster) in text.grapheme_indices(true) {
        while part + 1 < ends.len() && ends[part] <= at {
            part += 1;
        }
        let mut style = line.spans[part].style;
        let mut next = part + 1;
        while !marked(&style) && next < ends.len() && ends[next - 1] < at + cluster.len() {
            if marked(&line.spans[next].style) {
                style = line.spans[next].style;
            }
            next += 1;
        }
        if style != piece_style && !piece.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut piece), piece_style));
        }
        piece_style = style;
        piece.push_str(cluster);
    }
    if !piece.is_empty() {
        spans.push(Span::styled(piece, piece_style));
    }
    let mut out = Line::from(spans);
    out.style = line.style;
    out.alignment = line.alignment;
    out
}

#[cfg(test)]
thread_local! {
    /// The most cells one off-screen draw of this thread took.
    static DRAWN_CELLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The rows depend on the text of the spans only, never on their styles.
fn text_hash(line: &Line<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for span in &line.spans {
        span.content.as_ref().hash(&mut hasher);
    }
    hasher.finish()
}

/// The screen rows `line` takes at `width` cells.
fn counted_rows(line: Line<'static>, width: u16) -> usize {
    let count = |line| wrapped_row_count(line, width).clamp(1, MAX_LINE_ROWS);
    let line = whole_clusters(line);
    if line.spans.iter().map(|s| s.content.len()).sum::<usize>() < REMEMBERED_FROM {
        return count(line);
    }
    let key = (text_hash(&line), width);
    if let Some(n) = REMEMBERED.with(|m| m.borrow().get(&key).copied()) {
        return n;
    }
    let n = count(line);
    REMEMBERED.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= REMEMBERED_MOST {
            m.clear();
        }
        m.insert(key, n);
    });
    n
}

/// Where a wrapping window starts. The order is the reading order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    /// The first line shown (from 0).
    pub line: usize,
    /// The screen rows of that line already scrolled past.
    pub skip: usize,
}

/// The lines of a file read at once.
const CHUNK: usize = 64;

/// The lines of a windowed file by their number, found from the places already known: only the
/// lines around the ones asked for are read.
pub struct FileLines<'a> {
    win: &'a mut FileWindow,
    total: usize,
    /// (line, its line-head byte).
    known: Vec<(usize, u64)>,
    chunks: Vec<(usize, Vec<String>)>,
}

impl<'a> FileLines<'a> {
    /// A file of `total` lines in which the line `at.0` starts at the byte `at.1`.
    pub fn new(win: &'a mut FileWindow, total: usize, at: (usize, u64)) -> Self {
        Self {
            win,
            total,
            known: vec![at, (0, 0)],
            chunks: Vec::new(),
        }
    }

    /// The byte line `line` starts at (the length of the file past its last line).
    pub fn offset_of(&mut self, line: usize) -> u64 {
        let (at, off) = self
            .known
            .iter()
            .copied()
            .min_by_key(|(l, _)| l.abs_diff(line))
            .unwrap_or((0, 0));
        // The end is nearer than every known place: it is found by seeking back from the end.
        let from_end = self.total.saturating_sub(line);
        let found = if from_end > 0 && from_end < at.abs_diff(line) {
            self.win.last_page_top(from_end).unwrap_or(off)
        } else if line >= at {
            self.win.advance_kept(off, line - at).map_or(off, |r| r.0)
        } else {
            self.win.retreat_kept(off, at - line).map_or(off, |r| r.0)
        };
        self.known.push((line, found));
        found
    }

    /// The text of line `line` (empty past the end).
    pub fn get(&mut self, line: usize) -> String {
        let held = |(first, lines): &(usize, Vec<String>)| {
            line.checked_sub(*first).and_then(|i| lines.get(i)).cloned()
        };
        if let Some(s) = self.chunks.iter().find_map(held) {
            return s;
        }
        // A walk upwards reads the lines before, a walk downwards the lines after.
        let upwards = self.chunks.iter().any(|(first, _)| *first == line + 1);
        let first = if upwards {
            (line + 1).saturating_sub(CHUNK)
        } else {
            line
        };
        let off = self.offset_of(first);
        let lines = self.win.read_lines_kept(off, CHUNK).unwrap_or_default();
        let out = line
            .checked_sub(first)
            .and_then(|i| lines.get(i))
            .cloned()
            .unwrap_or_default();
        self.chunks.push((first, lines));
        out
    }
}

/// The byte a window stands at, `skip` of the `rows` rows into the line that starts at
/// `line_byte` (the next one at `next`): the rows count as shares of the line's bytes, for the
/// scrollbar and the position label.
pub fn byte_in_line(line_byte: u64, next: u64, skip: usize, rows: usize) -> u64 {
    let rows = rows.max(1) as u64;
    let share = (next.saturating_sub(line_byte) * (skip as u64).min(rows)).div_ceil(rows);
    line_byte + share
}

/// One line as it is drawn, and how many of its leading chars are the gutter.
pub struct Drawn {
    pub line: Line<'static>,
    pub gutter: usize,
}

/// Puts `gutter` (the host's line numbers and marks) on `body` and counts the chars it added.
pub fn drawn(body: Line<'static>, gutter: impl FnOnce(Line<'static>) -> Line<'static>) -> Drawn {
    let before = char_count(&body);
    let line = gutter(body);
    let gutter = char_count(&line).saturating_sub(before);
    Drawn { line, gutter }
}

fn char_count(line: &Line<'_>) -> usize {
    line.spans.iter().map(|s| s.content.chars().count()).sum()
}

/// The first `chars` chars of `line` (to the end of the cluster they stop in).
fn prefix(line: &Line<'static>, chars: usize) -> Line<'static> {
    let mut left = chars;
    let mut spans = Vec::new();
    for span in &line.spans {
        if left == 0 {
            break;
        }
        let mut end = 0usize;
        for (at, cluster) in span.content.grapheme_indices(true) {
            if left == 0 {
                break;
            }
            left = left.saturating_sub(cluster.chars().count());
            end = at + cluster.len();
        }
        spans.push(Span::styled(span.content[..end].to_string(), span.style));
    }
    Line::from(spans)
}

/// How far past a place the wrap still looks to decide the row of that place.
fn lookahead(width: u16) -> usize {
    4 * usize::from(width) + 16
}

/// The row of the char `target`. Only the text up to a little past it is drawn off screen, so
/// that asking about the start of a line of megabytes does not draw the whole of it.
fn row_holding(line: &Line<'static>, width: u16, target: usize) -> usize {
    let chars = target.saturating_add(lookahead(width));
    let starts = if chars < PIECE {
        row_starts(&prefix(line, chars), width)
    } else {
        // The whole line is asked about once and then remembered; every prefix would not be.
        row_starts(line, width)
    };
    starts
        .iter()
        .rposition(|s| s.is_some_and(|s| s <= target))
        .unwrap_or(0)
}

/// The [`row_starts`] of `line` as far as row `row` and one more, drawing no more of the line
/// than that takes.
fn starts_through(line: &Line<'static>, width: u16, row: usize) -> Vec<Option<usize>> {
    let chars = (row + 2)
        .saturating_mul(usize::from(width))
        .saturating_add(lookahead(width));
    if chars < PIECE && chars < char_count(line) {
        let starts = row_starts(&prefix(line, chars), width);
        // A later row began, so the rows asked about are as in the whole line.
        if starts.len() > row + 2 {
            return starts;
        }
    }
    row_starts(line, width)
}

/// The char index each screen row of `line` starts at when wrapped at `width` cells (`None`: no
/// char is drawn on that row). The line is drawn off screen, one color per grapheme, so the
/// answer is what the word wrap really does.
pub fn row_starts(line: &Line<'static>, width: u16) -> Vec<Option<usize>> {
    let line = whole_clusters(line.clone());
    if line.spans.iter().map(|s| s.content.len()).sum::<usize>() <= PIECE {
        return drawn_starts(&line, width);
    }
    let key = (text_hash(&line), width);
    let held = STARTS.with(|m| {
        let mut m = m.borrow_mut();
        let at = m.iter().position(|(k, _)| *k == key)?;
        let entry = m.remove(at);
        let starts = entry.1.clone();
        m.push(entry);
        Some(starts)
    });
    if let Some(starts) = held {
        return starts.to_vec();
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let starts = starts_in_pieces(&text, width, PIECE);
    STARTS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= REMEMBERED_STARTS {
            m.remove(0);
        }
        m.push((key, Rc::new(starts.clone())));
    });
    starts
}

/// [`row_starts`] of a long line, drawing about `piece` clusters at a time: each piece begins
/// where a row of the one before began, so the wrap goes on as in the whole line.
fn starts_in_pieces(text: &str, width: u16, piece: usize) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    let (mut from, mut base, mut size) = (0usize, 0usize, piece.max(1));
    while out.len() < MAX_LINE_ROWS {
        let rest = &text[from..];
        let end = rest
            .grapheme_indices(true)
            .nth(size)
            .map_or(rest.len(), |(at, _)| at);
        let starts = drawn_starts(&Line::raw(rest[..end].to_string()), width);
        let shifted = |s: &Option<usize>| s.map(|s| base + s);
        if end == rest.len() {
            out.extend(starts.iter().map(shifted));
            break;
        }
        // The last rows of a piece can differ: the text after them is missing. Begin again at the
        // start of a word where a row has one: a break inside a word depends on what came
        // before it.
        let chars: Vec<char> = rest[..end].chars().collect();
        let word = |s: usize| {
            chars.get(s).is_some_and(|c| !c.is_whitespace())
                && s.checked_sub(1)
                    .and_then(|b| chars.get(b))
                    .is_some_and(|c| c.is_whitespace())
        };
        let rows = || {
            (1..starts.len().saturating_sub(2))
                .rev()
                .filter_map(|k| starts[k].filter(|&s| s > 0).map(|s| (k, s)))
        };
        // A text with no word starts at all is cut inside a word once a few pieces held none.
        let patient = size < piece.max(WORD_REACH).saturating_mul(4);
        let cut = rows()
            .find(|&(_, s)| word(s))
            .or_else(|| rows().next().filter(|_| !patient));
        let Some((k, s)) = cut else {
            size *= 2;
            continue;
        };
        out.extend(starts[..k].iter().map(shifted));
        from += rest.char_indices().nth(s).map_or(rest.len(), |(at, _)| at);
        base += s;
        size = piece.max(1);
    }
    out.truncate(MAX_LINE_ROWS);
    out
}

/// [`row_starts`] of a line short enough to draw at once.
fn drawn_starts(line: &Line<'static>, width: u16) -> Vec<Option<usize>> {
    let rows = counted_rows(line.clone(), width);
    let mut spans = Vec::new();
    let mut index = 0usize;
    for span in &line.spans {
        let text: &str = &span.content;
        let mut byte = 0usize;
        for g in span.styled_graphemes(Style::default()) {
            let Some(at) = text[byte..].find(g.symbol) else {
                continue;
            };
            index += text[byte..byte + at].chars().count();
            let code = index.min(0x00ff_ffff) as u32;
            let color = Color::Rgb((code >> 16) as u8, (code >> 8) as u8, code as u8);
            spans.push(Span::styled(g.symbol.to_string(), Style::new().fg(color)));
            index += g.symbol.chars().count();
            byte += at + g.symbol.len();
        }
        index += text[byte..].chars().count();
    }
    let area = Rect::new(0, 0, width, rows as u16);
    #[cfg(test)]
    DRAWN_CELLS.with(|c| c.set(c.get().max(area.area() as usize)));
    // ratatui can write a wide char past a very narrow area, which must still be in the buffer.
    let mut buf = Buffer::empty(Rect::new(0, 0, width.saturating_add(8), area.height));
    Paragraph::new(Line::from(spans))
        .wrap(Wrap { trim: false })
        .render(area, &mut buf);
    (0..area.height)
        .map(|y| {
            (0..area.width).find_map(|x| match buf[(x, y)].fg {
                Color::Rgb(r, g, b) => {
                    Some((usize::from(r) << 16) | (usize::from(g) << 8) | usize::from(b))
                }
                _ => None,
            })
        })
        .collect()
}

/// A wrapping text of `total` lines in a view of `height` rows and `width` cells. `source(top,
/// line)` gives line `line` as it is drawn in a window that starts at line `top` (the width of a
/// line-number gutter depends on the window).
pub struct Wrapped<'a> {
    total: usize,
    height: usize,
    width: u16,
    source: &'a mut dyn FnMut(usize, usize) -> Drawn,
    memo: HashMap<(usize, usize), usize>,
}

impl<'a> Wrapped<'a> {
    pub fn new(
        total: usize,
        height: usize,
        width: u16,
        source: &'a mut dyn FnMut(usize, usize) -> Drawn,
    ) -> Self {
        Self {
            total,
            height: height.max(1),
            width: width.max(1),
            source,
            memo: HashMap::new(),
        }
    }

    /// The screen rows line `line` takes (at least one).
    pub fn rows(&mut self, top: usize, line: usize) -> usize {
        if line >= self.total {
            return 1;
        }
        if let Some(&n) = self.memo.get(&(top, line)) {
            return n;
        }
        let d = (self.source)(top, line);
        let n = counted_rows(d.line, self.width);
        self.memo.insert((top, line), n);
        n
    }

    /// The row of `line` that the char `col` of its text is drawn on. A column past the end is
    /// the last char.
    pub fn row_of(&mut self, line: usize, col: usize) -> usize {
        if line >= self.total || self.rows(line, line) == 1 {
            return 0;
        }
        let d = (self.source)(line, line);
        // With nothing before it the first char is on the first row; a gutter can push it down.
        if col == 0 && d.gutter == 0 {
            return 0;
        }
        let len = char_count(&d.line).saturating_sub(d.gutter);
        let target = d.gutter + col.min(len.saturating_sub(1));
        row_holding(&d.line, self.width, target)
    }

    /// The first column of the text drawn on row `row` of `line`.
    pub fn col_at(&mut self, line: usize, row: usize) -> usize {
        if line >= self.total || row == 0 {
            return 0;
        }
        let d = (self.source)(line, line);
        let starts = starts_through(&d.line, self.width, row);
        let from = row.min(starts.len().saturating_sub(1));
        starts[from..]
            .iter()
            .find_map(|s| *s)
            .or_else(|| starts[..from].iter().rev().find_map(|s| *s))
            .map_or(0, |s| s.saturating_sub(d.gutter))
    }

    /// The last window: the last row of the last line is on the bottom row of the view. A text
    /// that fits the view starts at its start.
    pub fn bottom(&mut self) -> Pos {
        // At the end the window reads up to the last line, whichever line it starts at.
        let top = self.total.saturating_sub(self.height);
        let mut have = 0usize;
        for line in (0..self.total).rev() {
            let rows = self.rows(top, line);
            if have + rows >= self.height {
                return Pos {
                    line,
                    skip: have + rows - self.height,
                };
            }
            have += rows;
        }
        Pos::default()
    }

    /// `pos` kept inside the text: never past [`Wrapped::bottom`], never past the rows of its
    /// line. Only the lines of one screen are counted (the end is read only when `pos` is past
    /// it).
    pub fn clamp(&mut self, pos: Pos) -> Pos {
        if self.total == 0 {
            return Pos::default();
        }
        let line = pos.line.min(self.total - 1);
        let rows = self.rows(line, line);
        let skip = pos.skip.min(rows - 1);
        let mut have = rows - skip;
        let mut next = line + 1;
        while have < self.height && next < self.total {
            have += self.rows(line, next);
            next += 1;
        }
        if have < self.height {
            self.bottom()
        } else {
            Pos { line, skip }
        }
    }

    /// `n` screen rows below `pos`, stopping at the last row of the text.
    pub fn walk_down(&mut self, pos: Pos, mut n: usize) -> Pos {
        let Pos { mut line, mut skip } = pos;
        while n > 0 {
            let rows = self.rows(pos.line, line);
            let left = rows.saturating_sub(1 + skip);
            if n <= left {
                skip += n;
                break;
            }
            if line + 1 >= self.total {
                skip += left;
                break;
            }
            n -= left + 1;
            line += 1;
            skip = 0;
        }
        Pos { line, skip }
    }

    /// `n` screen rows above `pos`, stopping at the start of the text.
    pub fn walk_up(&mut self, pos: Pos, mut n: usize) -> Pos {
        let Pos { mut line, mut skip } = pos;
        if n <= skip {
            return Pos {
                line,
                skip: skip - n,
            };
        }
        n -= skip;
        skip = 0;
        while n > 0 && line > 0 {
            line -= 1;
            let rows = self.rows(line, line);
            if n <= rows {
                skip = rows - n;
                break;
            }
            n -= rows;
        }
        Pos { line, skip }
    }

    /// The row of the view that row `row` of `line` is on when the window is at `pos`, or None
    /// when it is not on screen.
    pub fn screen_row(&mut self, pos: Pos, line: usize, row: usize) -> Option<usize> {
        if (Pos { line, skip: row }) < pos {
            return None;
        }
        let y = if line == pos.line {
            row - pos.skip
        } else {
            let mut have = self.rows(pos.line, pos.line).saturating_sub(pos.skip);
            for between in pos.line + 1..line {
                if have >= self.height {
                    return None;
                }
                have += self.rows(pos.line, between);
            }
            have + row
        };
        (y < self.height).then_some(y)
    }

    /// The window after the caret moved to line `line` (`j` / `k`, a jump to a line): the whole
    /// line is brought on screen, moving as little as possible. A line longer than the view is
    /// shown from its start (or as far down as keeps the caret's row on screen).
    pub fn follow_line(&mut self, pos: Pos, line: usize, col: usize) -> Pos {
        let rows = self.rows(line, line);
        let row = self.row_of(line, col);
        let next = if rows <= self.height {
            let start = Pos { line, skip: 0 };
            if start < pos {
                start
            } else if self.screen_row(pos, line, rows - 1).is_none() {
                self.walk_up(
                    Pos {
                        line,
                        skip: rows - 1,
                    },
                    self.height - 1,
                )
            } else {
                pos
            }
        } else if pos.line == line {
            return self.follow_row(pos, line, col);
        } else {
            Pos {
                line,
                skip: row.saturating_sub(self.height - 1),
            }
        };
        self.clamp(next)
    }

    /// The window moved as little as keeps the row of the char `col` of line `line` on screen
    /// (the column keys, a search match).
    pub fn follow_row(&mut self, pos: Pos, line: usize, col: usize) -> Pos {
        let row = self.row_of(line, col);
        let at = Pos { line, skip: row };
        let next = if at < pos {
            at
        } else if self.screen_row(pos, line, row).is_none() {
            self.walk_up(at, self.height - 1)
        } else {
            pos
        };
        self.clamp(next)
    }

    /// A page or a half page of `delta` screen rows: the window moves and the caret keeps its row
    /// of the view (the column moves only when it would be on another row of its line). At either
    /// end, where the window cannot move, the caret stays. Returns the window, the line and the
    /// column.
    pub fn page(&mut self, pos: Pos, line: usize, col: usize, delta: i32) -> (Pos, usize, usize) {
        let row = self.row_of(line, col);
        let y = if (Pos { line, skip: row }) < pos {
            0
        } else {
            self.screen_row(pos, line, row).unwrap_or(self.height - 1)
        };
        let n = delta.unsigned_abs() as usize;
        let moved = if delta > 0 {
            self.walk_down(pos, n)
        } else {
            self.walk_up(pos, n)
        };
        let next = self.clamp(moved);
        if next == pos {
            return (pos, line, col);
        }
        let target = self.walk_down(next, y);
        let col = if self.row_of(target.line, col) == target.skip {
            col
        } else {
            self.col_at(target.line, target.skip)
        };
        (next, target.line, col)
    }

    /// `g`: the start of the text. A column that would be below the view returns to the first.
    pub fn to_top(&mut self, col: usize) -> (Pos, usize) {
        let col = if self.row_of(0, col) < self.height {
            col
        } else {
            0
        };
        (Pos::default(), col)
    }

    /// `G`: the last window and the caret on the last line, on a row of it that is on screen.
    /// Returns the window, the line and the column.
    pub fn to_bottom(&mut self, col: usize) -> (Pos, usize, usize) {
        let pos = self.bottom();
        let line = self.total.saturating_sub(1);
        let col = if pos.line == line && self.row_of(line, col) < pos.skip {
            self.col_at(line, pos.skip)
        } else {
            col
        };
        (pos, line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source over `lines` with no gutter.
    fn plain(lines: &[&str]) -> impl FnMut(usize, usize) -> Drawn {
        let lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        move |_, i| Drawn {
            line: Line::from(lines[i].clone()),
            gutter: 0,
        }
    }

    /// The rows of the screen as text, drawn the way the hosts draw a window.
    fn screen(lines: &[&str], pos: Pos, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let body: Vec<Line> = lines[pos.line..]
            .iter()
            .map(|s| Line::from(s.to_string()))
            .collect();
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll((pos.skip as u16, 0))
            .render(area, &mut buf);
        (0..height)
            .map(|y| {
                let row: String = (0..width).map(|x| buf[(x, y)].symbol()).collect();
                row.trim_end().to_string()
            })
            .collect()
    }

    fn at(line: usize, skip: usize) -> Pos {
        Pos { line, skip }
    }

    /// Drawing only up to a little past the place answers as drawing the whole line does.
    /// Drawing a long line piece by piece finds the rows that drawing it at once finds.
    #[test]
    fn pieces_find_the_rows_of_the_whole_line() {
        let texts = [
            "alpha bravo charlie delta ".repeat(40),
            "abcdefghijklmnopqrstuvwxyz0123456789".repeat(20),
            "全角の長い文章がここに続きます。 ab ".repeat(30),
            format!("{}word{}end", " ".repeat(300), "          next ".repeat(30)),
            "ab\u{2764}\u{fe0f} e\u{301}x \u{1f468}\u{200d}\u{1f469} ".repeat(40),
            format!("a {} b", "x".repeat(500)),
            "a  bb   ccc    dddd     eeeeeeeeeeeeeeeeeeeeeeeee ".repeat(20),
            "漢a 字 bb漢字漢字漢字漢字 c".repeat(30),
            "全角の長い文章がここに続きます".repeat(30),
            "a漢字bc漢d".repeat(60),
        ];
        let mut checked = 0;
        for text in &texts {
            for width in [1u16, 2, 3, 4, 5, 7, 20, 41] {
                let whole = drawn_starts(&Line::raw(text.clone()), width);
                for piece in [1usize, 17, 64, 3 * usize::from(width) + 5, 301] {
                    assert_eq!(
                        starts_in_pieces(text, width, piece),
                        whole,
                        "{width} piece {piece} {text:.30}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 400);
    }

    /// The end of a line of megabytes is found a piece at a time, once.
    #[test]
    fn asking_near_the_end_of_a_huge_line_draws_a_piece_at_a_time() {
        let line = Line::raw("word ".repeat(200_000));
        let mut source = |_: usize, _: usize| Drawn {
            line: line.clone(),
            gutter: 0,
        };
        let mut w = Wrapped::new(1, 20, 78, &mut source);
        DRAWN_CELLS.with(|c| c.set(0));
        let last = w.row_of(0, usize::MAX);
        assert_eq!(last + 1, w.rows(0, 0));
        let cells = DRAWN_CELLS.with(std::cell::Cell::get);
        assert!(cells > 0 && cells < 4 * PIECE, "{cells}");
        DRAWN_CELLS.with(|c| c.set(0));
        assert_eq!(w.row_of(0, usize::MAX), last);
        assert!(w.col_at(0, last) > 999_000);
        assert_eq!(DRAWN_CELLS.with(std::cell::Cell::get), 0, "remembered");
    }

    #[test]
    fn a_prefix_answers_as_the_whole_line_does() {
        let texts = [
            "alpha bravo charlie delta ".repeat(40),
            "abcdefghijklmnopqrstuvwxyz0123456789".repeat(20),
            "全角の長い文章がここに続きます。 ab ".repeat(30),
            format!("{}word{}end", " ".repeat(300), "          next ".repeat(30)),
            "ab\u{2764}\u{fe0f} e\u{301}x \u{1f468}\u{200d}\u{1f469} ".repeat(40),
            format!("a {} b", "x".repeat(500)),
        ];
        let mut checked = 0;
        for text in &texts {
            for width in [1u16, 2, 7, 20, 41] {
                let line = whole_clusters(Line::raw(text.clone()));
                let whole = row_starts(&line, width);
                let chars = text.chars().count();
                for target in (0..chars).step_by(13) {
                    let want = whole
                        .iter()
                        .rposition(|s| s.is_some_and(|s| s <= target))
                        .unwrap_or(0);
                    assert_eq!(
                        row_holding(&line, width, target),
                        want,
                        "{width} {target} {text:.20}"
                    );
                    checked += 1;
                }
                for row in (0..whole.len()).step_by(3) {
                    let got = starts_through(&line, width, row);
                    assert_eq!(got[..=row], whole[..=row], "{width} row {row} {text:.20}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 3000, "{checked}");
    }

    /// A question near the start of a line of megabytes draws a few rows, not the line.
    #[test]
    fn asking_near_the_start_of_a_huge_line_draws_little() {
        let line = Line::raw("word ".repeat(1_000_000));
        let mut source = |_: usize, _: usize| Drawn {
            line: line.clone(),
            gutter: 0,
        };
        let mut w = Wrapped::new(1, 20, 78, &mut source);
        DRAWN_CELLS.with(|c| c.set(0));
        assert_eq!(w.row_of(0, 100), 1);
        assert!(w.col_at(0, 3) > 200);
        let cells = DRAWN_CELLS.with(std::cell::Cell::get);
        assert!(cells > 0 && cells < 78 * 40, "{cells}");
    }

    #[test]
    fn row_starts_follow_the_word_wrap_not_a_division_of_chars() {
        // Words move whole to the next row, so row 1 starts at "bbbbbb", not at char 10.
        let line = Line::from("aaaa bbbbbbb cc".to_string());
        assert_eq!(row_starts(&line, 10), vec![Some(0), Some(5)]);
        // Wide characters: two cells each.
        let cjk = Line::from("あいうえおかき".to_string());
        assert_eq!(row_starts(&cjk, 6), vec![Some(0), Some(3), Some(6)]);
        // Several spans, an emoji sequence in one of them.
        let spans = Line::from(vec![Span::raw("ab"), Span::raw("👨\u{200d}👩cdef")]);
        assert_eq!(row_starts(&spans, 4)[0], Some(0));
        assert_eq!(
            row_starts(&spans, 4).len(),
            wrapped_row_count(spans.clone(), 4)
        );
    }

    #[test]
    fn a_remembered_long_line_is_counted_per_text_and_width() {
        let a = Line::from("word ".repeat(400));
        let b = Line::from("words ".repeat(400));
        for line in [&a, &b, &a, &b] {
            for width in [10u16, 40, 10] {
                assert_eq!(
                    counted_rows(line.clone(), width),
                    wrapped_row_count(line.clone(), width)
                );
            }
        }
    }

    #[test]
    fn row_of_and_col_at_agree_and_account_for_the_gutter() {
        let mut src = |_: usize, _: usize| {
            drawn(Line::from("abcdefghij".to_string()), |l| {
                let mut spans = vec![Span::raw("12 ")];
                spans.extend(l.spans);
                Line::from(spans)
            })
        };
        // "12 " then ten chars in 8 cells: the word does not fit after the gutter.
        let mut w = Wrapped::new(1, 5, 8, &mut src);
        let rows = w.rows(0, 0);
        assert!(rows > 1);
        for row in 0..rows {
            let col = w.col_at(0, row);
            if row > 0 {
                assert_eq!(w.row_of(0, col), row, "col {col}");
            }
        }
        assert_eq!(w.row_of(0, usize::MAX), rows - 1);
        // The first char is on the second row, below the gutter.
        assert_eq!(w.row_of(0, 0), 1);
        assert_eq!(w.col_at(0, 0), 0);
    }

    const TEXT: [&str; 6] = [
        "L0",
        "L1 aaaa bbbb cccc",
        "L2",
        "漢字漢字漢字漢字",
        "L4",
        "L5",
    ];

    #[test]
    fn the_bottom_window_ends_on_the_last_row() {
        let mut src = plain(&TEXT);
        for height in 1..=12u16 {
            let pos = Wrapped::new(TEXT.len(), height.into(), 6, &mut src).bottom();
            let shown = screen(&TEXT, pos, 6, height);
            let total: usize = (0..TEXT.len())
                .map(|i| wrapped_row_count(Line::from(TEXT[i]), 6))
                .sum();
            if total >= usize::from(height) {
                assert_eq!(shown.last().unwrap(), "L5", "height {height}: {shown:?}");
            } else {
                assert_eq!(pos, Pos::default());
            }
        }
    }

    #[test]
    fn without_wrapping_lines_it_is_the_line_arithmetic() {
        let lines: Vec<String> = (0..20).map(|i| format!("l{i}")).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut src = plain(&lines);
        let mut w = Wrapped::new(20, 5, 30, &mut src);
        let mut c = crate::caret::Caret::default();
        let mut pos = Pos::default();
        let mut line = 0usize;
        // Steps, pages and the ends give what `Caret` gives.
        for delta in [1i32, 1, 1, 1, 1, 1, 7, -2, -9, 30, -30] {
            c.step(delta, 20, 5);
            line = (line as i64 + i64::from(delta)).clamp(0, 19) as usize;
            pos = w.follow_line(pos, line, 0);
            assert_eq!(
                (pos.line, pos.skip, line),
                (c.top, 0, c.line),
                "step {delta}"
            );
        }
        for delta in [4i32, 4, 2, 4, 4, 4, 4, -4, -2, -30, -4] {
            c.page(delta, 20, 5);
            let next = w.page(pos, line, 0, delta);
            pos = next.0;
            line = next.1;
            assert_eq!(
                (pos.line, pos.skip, line),
                (c.top, 0, c.line),
                "page {delta}"
            );
        }
        c.to_bottom(20, 5);
        assert_eq!(w.to_bottom(0).0, at(c.top, 0));
    }

    #[test]
    fn follow_line_shows_the_whole_line_or_the_start_of_a_longer_one() {
        let long = "word ".repeat(20);
        let text = ["a", "b", "c", "d e f g h i j k", &long, "z"];
        let mut src = plain(&text);
        let mut w = Wrapped::new(text.len(), 4, 5, &mut src);
        // Line 3 takes 4 rows ("d e", "f g", ...): the whole line comes into view.
        let rows3 = w.rows(0, 3);
        assert!(rows3 > 1 && rows3 <= 4, "{rows3}");
        let pos = w.follow_line(Pos::default(), 3, 0);
        assert!(w.screen_row(pos, 3, 0).is_some());
        assert!(w.screen_row(pos, 3, rows3 - 1).is_some());
        // Line 4 is longer than the view: it is shown from its start.
        assert_eq!(w.follow_line(pos, 4, 0), at(4, 0));
        // A column on a lower row of it moves the window within the line.
        let p = w.follow_row(at(4, 0), 4, usize::MAX);
        assert_eq!(p.line, 4);
        let last = w.row_of(4, usize::MAX);
        assert!(w.screen_row(p, 4, last).is_some());
        // And back up.
        assert_eq!(w.follow_row(p, 4, 0), at(4, 0));
    }

    #[test]
    fn paging_reads_all_of_a_line_longer_than_the_view() {
        let long: String = (0..40).map(|i| format!("w{i:02} ")).collect();
        let text = [long.as_str(), "end"];
        let mut src = plain(&text);
        let mut w = Wrapped::new(2, 4, 8, &mut src);
        let mut pos = Pos::default();
        let (mut line, mut col) = (0usize, 0usize);
        let mut seen = String::new();
        for _ in 0..40 {
            seen.push_str(&screen(&text, pos, 8, 4).join(" "));
            seen.push(' ');
            // The caret is on screen at every step.
            let row = w.row_of(line, col);
            assert!(
                w.screen_row(pos, line, row).is_some(),
                "{pos:?} {line} {col}"
            );
            let next = w.page(pos, line, col, 3);
            if next.0 == pos {
                break;
            }
            (pos, line, col) = next;
        }
        for i in 0..40 {
            assert!(seen.contains(&format!("w{i:02}")), "w{i:02}");
        }
        assert!(seen.contains("end"));
        assert_eq!(pos, w.bottom());
    }

    #[test]
    fn to_bottom_and_to_top_keep_the_caret_on_screen_in_a_long_line() {
        let long = "word ".repeat(30);
        let text = ["a", long.as_str()];
        let mut src = plain(&text);
        let mut w = Wrapped::new(2, 3, 10, &mut src);
        let (pos, line, col) = w.to_bottom(0);
        assert_eq!(line, 1);
        assert_eq!(pos, w.bottom());
        let row = w.row_of(line, col);
        assert!(w.screen_row(pos, line, row).is_some(), "{pos:?} {col}");
        let (pos, col) = w.to_top(usize::MAX);
        assert_eq!((pos, col), (Pos::default(), usize::MAX));
    }

    #[test]
    fn clamp_stops_at_the_end_and_inside_a_line() {
        let text = ["aaaa aaaa aaaa", "b", "c"];
        let mut src = plain(&text);
        let mut w = Wrapped::new(3, 3, 4, &mut src);
        assert_eq!(w.clamp(at(2, 9)), w.bottom());
        assert_eq!(w.clamp(at(0, 1)), at(0, 1));
        assert_eq!(w.clamp(at(99, 0)), w.bottom());
        let mut none = plain(&[]);
        assert_eq!(
            Wrapped::new(0, 3, 4, &mut none).clamp(at(1, 1)),
            Pos::default()
        );
    }
}
