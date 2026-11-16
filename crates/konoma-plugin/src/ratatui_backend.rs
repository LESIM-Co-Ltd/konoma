//! A backend wrapper that writes wide cells where the terminal really is.
//!
//! ratatui-core 0.1.2 emits the covered (trailing) cells of a wide cell whose symbol holds VS16
//! (U+FE0F) right *after* the wide cell, and ratatui-crossterm 0.1.2 skips the cursor move when a
//! cell sits at `last.x + 1`. Terminals disagree on how wide such an emoji is (tmux draws it one
//! column wide by default, Ghostty / kitty / WezTerm two), so after it the cursor is at an unknown
//! column: the blank written for the covered cell lands on the next character and erases it, and
//! the buffer (which still holds that character unchanged) never repairs it.
//!
//! [`WideSafeBackend`] delegates everything to the inner backend except `draw`, which
//! 1. writes the cells inside the span of a VS16 wide cell *before* that cell (ratatui#2686), and
//! 2. forces a cursor move wherever the cursor is not known to be at the cell (the previous cell is
//!    on the same row, holds no VS16 and ends exactly at this `x`; ratatui#2721), by splitting the
//!    cells into runs and calling the inner `draw` once per run (each call starts with a `MoveTo`).
//!
//! Splitting is safe because every inner `draw` of the crossterm backend starts from the reset
//! state (fg, bg, modifier and underline colour are all `Reset` / empty at the top) and ends by
//! resetting all of them, so a run never depends on the style the previous run left behind. The one
//! place that uses this wrapper for every terminal konoma draws to is the host terminal and the
//! standalone app terminal; it lives next to `ratatui_draw` because both turn the `ratatui`
//! feature on.

use std::io::{self, Stdout, Write};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Position, Size};

/// The terminal backend both hosts draw through.
pub type HostBackend = WideSafeBackend<CrosstermBackend<Stdout>>;
/// The terminal both hosts draw to.
pub type HostTerminal = ratatui::Terminal<HostBackend>;

/// A backend that draws like its inner one, except for the wide cells described in the module doc.
#[derive(Debug)]
pub struct WideSafeBackend<B: Backend> {
    inner: B,
}

impl<B: Backend> WideSafeBackend<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &B {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut B {
        &mut self.inner
    }
}

impl<B: Backend + Write> Write for WideSafeBackend<B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

fn has_vs16(cell: &Cell) -> bool {
    cell.symbol().contains('\u{FE0F}')
}

type Item<'a> = (u16, u16, &'a Cell);

/// The cells in the order they are written: the cells inside the span of a VS16 wide cell come
/// before that cell.
fn reorder<'a>(content: impl Iterator<Item = Item<'a>>) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    let mut it = content.peekable();
    while let Some((x, y, cell)) = it.next() {
        let width = cell.cell_width();
        if width > 1 && has_vs16(cell) {
            let end = x.saturating_add(width);
            while let Some(&(nx, ny, ncell)) = it.peek() {
                if ny == y && nx > x && nx < end {
                    out.push((nx, ny, ncell));
                    it.next();
                } else {
                    break;
                }
            }
        }
        out.push((x, y, cell));
    }
    out
}

/// Whether the terminal cursor is known to be at `cur` after `prev` was written.
fn cursor_known(prev: Item<'_>, cur: Item<'_>) -> bool {
    let (px, py, pcell) = prev;
    let (x, y, _) = cur;
    py == y && !has_vs16(pcell) && u32::from(px) + u32::from(pcell.cell_width()) == u32::from(x)
}

/// Whether the inner backend would skip the cursor move before `cur` although the cursor is not
/// known to be there (it skips when `cur` is at `prev.x + 1` on the same row).
fn needs_new_run(prev: Item<'_>, cur: Item<'_>) -> bool {
    let would_skip = prev.1 == cur.1 && u32::from(prev.0) + 1 == u32::from(cur.0);
    would_skip && !cursor_known(prev, cur)
}

impl<B: Backend> Backend for WideSafeBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells = reorder(content);
        let mut start = 0;
        for i in 1..cells.len() {
            if needs_new_run(cells[i - 1], cells[i]) {
                self.inner.draw(cells[start..i].iter().copied())?;
                start = i;
            }
        }
        self.inner.draw(cells[start..].iter().copied())
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::{Buffer, CellDiffOption};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    const HEART: &str = "\u{2764}\u{fe0f}";

    /// A terminal that consumes the bytes a `CrosstermBackend<Vec<u8>>` writes. `vs16_width` is how
    /// wide the terminal draws a cluster holding U+FE0F (tmux: 1; Ghostty, kitty, WezTerm: 2).
    /// Overwriting either half of a wide character erases the other half.
    struct Model {
        vs16_width: usize,
        /// (symbol, covered by the wide character before it, SGR parameters in force)
        rows: Vec<Vec<(String, bool, String)>>,
        col: usize,
        row: usize,
        fg: String,
        bg: String,
        moves: usize,
    }

    impl Model {
        fn new(w: usize, h: usize, vs16_width: usize) -> Self {
            Self {
                vs16_width,
                rows: vec![vec![(" ".into(), false, String::new()); w]; h],
                col: 0,
                row: 0,
                fg: String::new(),
                bg: String::new(),
                moves: 0,
            }
        }

        fn width_of(&self, cluster: &str) -> usize {
            if cluster.contains('\u{fe0f}') {
                self.vs16_width
            } else {
                cluster.width().max(1)
            }
        }

        fn put(&mut self, cluster: &str) {
            let w = self.width_of(cluster);
            let (x, sgr) = (self.col, format!("{}/{}", self.fg, self.bg));
            if self.row < self.rows.len() && x < self.rows[self.row].len() {
                let row = &mut self.rows[self.row];
                if row[x].1 && x > 0 {
                    row[x - 1] = (" ".into(), false, String::new());
                }
                if row[x].0.contains('\u{fe0f}') || row[x].0.width() > 1 {
                    // The first half of a wide character is overwritten: its second half goes.
                    for k in 1..2 {
                        if x + k < row.len() && row[x + k].1 {
                            row[x + k] = (" ".into(), false, String::new());
                        }
                    }
                }
                row[x] = (cluster.to_string(), false, sgr.clone());
                for k in 1..w {
                    if x + k < row.len() {
                        if (row[x + k].1 || row[x + k].0.width() > 1)
                            && x + k + 1 < row.len()
                            && row[x + k + 1].1
                        {
                            row[x + k + 1] = (" ".into(), false, String::new());
                        }
                        row[x + k] = (" ".into(), true, sgr.clone());
                    }
                }
            }
            self.col += w;
        }

        fn feed(&mut self, bytes: &[u8]) {
            let text = String::from_utf8_lossy(bytes).into_owned();
            let mut rest = text.as_str();
            while !rest.is_empty() {
                if let Some(csi) = rest.strip_prefix("\u{1b}[") {
                    let end = csi.find(|c: char| c.is_ascii_alphabetic()).unwrap();
                    let (params, fin) = (&csi[..end], csi.as_bytes()[end] as char);
                    match fin {
                        'H' => {
                            let mut p = params.split(';');
                            self.row = p.next().unwrap().parse::<usize>().unwrap() - 1;
                            self.col = p.next().unwrap().parse::<usize>().unwrap() - 1;
                            self.moves += 1;
                        }
                        'm' => {
                            let t: Vec<&str> = params.split(';').collect();
                            let mut i = 0;
                            while i < t.len() {
                                let n: u32 = t[i].parse().unwrap_or(0);
                                let take = |i: usize| match t.get(i + 1) {
                                    Some(&"5") => (3, t[i..i + 3].join(";")),
                                    _ => (5, t[i..i + 5].join(";")),
                                };
                                match n {
                                    0 if params.is_empty() || t[i] == "0" => {
                                        self.fg.clear();
                                        self.bg.clear();
                                    }
                                    38 | 48 => {
                                        let (len, text) = take(i);
                                        if n == 38 {
                                            self.fg = text;
                                        } else {
                                            self.bg = text;
                                        }
                                        i += len - 1;
                                    }
                                    39 => self.fg.clear(),
                                    49 => self.bg.clear(),
                                    30..=37 | 90..=97 => self.fg = t[i].to_string(),
                                    40..=47 | 100..=107 => self.bg = t[i].to_string(),
                                    _ => {}
                                }
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    rest = &csi[end + 1..];
                } else {
                    let end = rest.find('\u{1b}').unwrap_or(rest.len());
                    for g in rest[..end].graphemes(true) {
                        self.put(g);
                    }
                    rest = &rest[end..];
                }
            }
        }

        /// The row as the terminal shows it: covered halves are not shown.
        fn row_text(&self, y: usize) -> String {
            self.rows[y]
                .iter()
                .filter(|(_, covered, _)| !covered)
                .map(|(s, _, _)| s.as_str())
                .collect()
        }
    }

    /// The writer the backend under test writes to; the test reads the bytes back.
    #[derive(Clone, Default)]
    struct Sink(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A terminal over `CrosstermBackend<Sink>` (through the wrapper or not) that hands the
    /// bytes of each frame to a model.
    fn draw_frames(
        wrapped: bool,
        vs16_width: usize,
        width: u16,
        frames: &[Buffer],
    ) -> (Model, Vec<Vec<u8>>) {
        let mut model = Model::new(usize::from(width), 1, vs16_width);
        let mut written = Vec::new();
        let area = Rect::new(0, 0, width, 1);
        let paint = |f: &mut ratatui::Frame, frame: &Buffer| {
            *f.buffer_mut() = frame.clone();
        };
        // The viewport is fixed so no cursor query (a real tty) is made.
        let options = ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(area),
        };
        let sink = Sink::default();
        let mut take = |model: &mut Model| {
            let bytes = std::mem::take(&mut *sink.0.borrow_mut());
            model.feed(&bytes);
            written.push(bytes);
        };
        if wrapped {
            let mut term = Terminal::with_options(
                WideSafeBackend::new(CrosstermBackend::new(sink.clone())),
                options,
            )
            .unwrap();
            for frame in frames {
                term.draw(|f| paint(f, frame)).unwrap();
                take(&mut model);
            }
        } else {
            let mut term =
                Terminal::with_options(CrosstermBackend::new(sink.clone()), options).unwrap();
            for frame in frames {
                term.draw(|f| paint(f, frame)).unwrap();
                take(&mut model);
            }
        }
        (model, written)
    }

    /// The review's reproduction: frame 1 has the heart blanked by the end-of-frame normalisation
    /// because a skipped (image) cell owns its span; frame 2 has the heart back.
    fn review_frames() -> [Buffer; 2] {
        let area = Rect::new(0, 0, 6, 1);
        let mut one = Buffer::empty(area);
        one[(0, 0)].set_symbol(HEART);
        one[(1, 0)]
            .set_symbol("x")
            .set_diff_option(CellDiffOption::Skip);
        one[(2, 0)].set_symbol("Z");
        crate::ratatui_draw::clear_cells_hidden_by_wide_symbols(&mut one);
        let mut two = Buffer::empty(area);
        two[(0, 0)].set_symbol(HEART);
        two[(2, 0)].set_symbol("Z");
        crate::ratatui_draw::clear_cells_hidden_by_wide_symbols(&mut two);
        [one, two]
    }

    /// The row the buffer describes as a terminal with this VS16 width draws it.
    fn expected_row(frame: &Buffer, vs16_width: usize) -> String {
        let mut out = String::new();
        let mut x = 0;
        while x < usize::from(frame.area.width) {
            let sym = frame[(x as u16, 0)].symbol();
            out.push_str(sym);
            x += if sym.contains('\u{fe0f}') {
                vs16_width
            } else {
                sym.width().max(1)
            };
        }
        out
    }

    #[test]
    fn the_review_repro_keeps_the_character_after_a_vs16_emoji() {
        for vs16_width in [2, 1] {
            let frames = review_frames();
            let (model, _) = draw_frames(true, vs16_width, 6, &frames);
            assert_eq!(
                model.row_text(0).trim_end(),
                expected_row(&frames[1], vs16_width).trim_end(),
                "vs16 width {vs16_width}"
            );
            let z = model.rows[0].iter().position(|c| c.0 == "Z").unwrap();
            assert_eq!(z, 2, "Z stays at column 2 (vs16 width {vs16_width})");
        }
    }

    #[test]
    fn without_the_wrapper_the_review_repro_loses_the_character() {
        let frames = review_frames();
        let (model, _) = draw_frames(false, 2, 6, &frames);
        assert!(
            model.rows[0].iter().all(|c| c.0 != "Z"),
            "the inner backend alone erases Z: {:?}",
            model.row_text(0)
        );
    }

    #[test]
    fn text_changed_under_a_vs16_emoji_needs_no_image_to_lose_the_character() {
        let area = Rect::new(0, 0, 6, 1);
        let mut one = Buffer::empty(area);
        one[(0, 0)].set_symbol("a");
        one[(1, 0)].set_symbol("q");
        one[(2, 0)].set_symbol("Z");
        let mut two = Buffer::empty(area);
        two[(0, 0)].set_symbol(HEART);
        two[(2, 0)].set_symbol("Z");
        let frames = [one, two];
        let (plain, _) = draw_frames(false, 2, 6, &frames);
        assert!(
            plain.rows[0].iter().all(|c| c.0 != "Z"),
            "the inner backend alone loses Z"
        );
        for vs16_width in [2, 1] {
            let (model, _) = draw_frames(true, vs16_width, 6, &frames);
            assert_eq!(
                model.row_text(0).trim_end(),
                expected_row(&frames[1], vs16_width).trim_end(),
                "vs16 width {vs16_width}"
            );
            assert!(model.rows[0].iter().any(|c| c.0 == "Z"));
        }
    }

    #[test]
    fn a_cell_right_after_a_vs16_cluster_gets_its_own_cursor_move() {
        // "a" + VS16 is one column wide for the buffer, but a terminal may draw it two wide.
        let area = Rect::new(0, 0, 6, 1);
        let mut frame = Buffer::empty(area);
        frame[(0, 0)].set_symbol("a\u{fe0f}");
        frame[(1, 0)].set_symbol("b");
        for vs16_width in [2, 1] {
            let (model, _) = draw_frames(true, vs16_width, 6, std::slice::from_ref(&frame));
            assert_eq!(model.rows[0][1].0, "b", "vs16 width {vs16_width}");
        }
    }

    #[test]
    fn cjk_runs_are_written_with_no_move_the_inner_backend_would_not_make() {
        let area = Rect::new(0, 0, 8, 1);
        let mut frame = Buffer::empty(area);
        for (x, s) in [(0, "変"), (2, "a"), (3, "b"), (4, "漢"), (6, "c")] {
            frame[(x, 0)].set_symbol(s);
        }
        let (plain, plain_bytes) = draw_frames(false, 2, 8, std::slice::from_ref(&frame));
        let (wrapped, wrapped_bytes) = draw_frames(true, 2, 8, std::slice::from_ref(&frame));
        assert_eq!(wrapped.moves, plain.moves);
        assert_eq!(wrapped_bytes, plain_bytes, "no run is split without a VS16");
        assert_eq!(wrapped.row_text(0).trim_end(), "変ab漢c");
    }

    #[test]
    fn a_coloured_cell_after_a_vs16_emoji_keeps_its_colour() {
        let area = Rect::new(0, 0, 6, 1);
        let mut frame = Buffer::empty(area);
        frame[(0, 0)].set_symbol(HEART).set_fg(Color::Blue);
        frame[(1, 0)].set_symbol("q");
        frame[(2, 0)].set_symbol("R").set_fg(Color::Red);
        frame[(3, 0)].set_symbol("g").set_fg(Color::Green);
        // The reference: the same cells where no split happens.
        let mut plain = Buffer::empty(area);
        plain[(2, 0)].set_symbol("R").set_fg(Color::Red);
        plain[(3, 0)].set_symbol("g").set_fg(Color::Green);
        let (reference, _) = draw_frames(false, 2, 6, &[plain]);
        let (model, _) = draw_frames(true, 2, 6, &[frame]);
        for x in [2, 3] {
            assert!(
                model.rows[0][x].2.starts_with("38;5;"),
                "column {x} is coloured"
            );
            assert_eq!(model.rows[0][x].2, reference.rows[0][x].2, "column {x}");
        }
        assert_ne!(model.rows[0][2].2, model.rows[0][3].2);
        assert_eq!(model.rows[0][0].2, reference_blue(), "the heart is blue");
    }

    /// The SGR parameters crossterm writes for a blue foreground over the default background.
    fn reference_blue() -> String {
        let mut b = Buffer::empty(Rect::new(0, 0, 2, 1));
        b[(0, 0)].set_symbol("x").set_fg(Color::Blue);
        let (m, _) = draw_frames(false, 2, 2, &[b]);
        m.rows[0][0].2.clone()
    }
}
