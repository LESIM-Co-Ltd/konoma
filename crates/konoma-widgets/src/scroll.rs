//! The scroll position of a bordered view: the `All` / `Top` / `Bot` / `NN%` label on the top
//! border and the thumb drawn over the right border column. The konoma host and the standalone
//! apps draw them with this one code.

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// Where a scrolling view is: the position, the end of the range and the size of one screenful.
/// The label and the scrollbar read only the *ratios* between these three numbers, never their
/// absolute values, so a view may count display rows or bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollExtent {
    /// Current offset from the start of the content.
    pub pos: u64,
    /// Largest reachable offset. `0` means everything already fits on one screen.
    pub max: u64,
    /// How much of the content one screenful covers, in the same unit.
    pub viewport: u64,
}

impl ScrollExtent {
    /// `pos` is clamped into `0..=max` so "at the end" is decided the same way everywhere, even
    /// when a caller passes a position that a resize has not been reconciled with yet.
    pub fn new(pos: u64, max: u64, viewport: u64) -> Self {
        Self {
            pos: pos.min(max),
            max,
            viewport,
        }
    }
}

/// The words of the label, in the reader's language.
#[derive(Debug, Clone, Copy)]
pub struct ScrollWords {
    pub all: &'static str,
    pub top: &'static str,
    pub bot: &'static str,
}

/// Position label with vim's `%P` semantics: `All` when nothing is off-screen, `Top`/`Bot` at the
/// two ends, and a percentage of the **scrollable range** in between.
pub fn scroll_label(e: ScrollExtent, w: ScrollWords) -> String {
    if e.max == 0 {
        w.all.to_string()
    } else if e.pos == 0 {
        w.top.to_string()
    } else if e.pos >= e.max {
        w.bot.to_string()
    } else {
        format!("{}%", e.pos * 100 / e.max)
    }
}

/// The ` [Bot] ` marker as a **right-aligned** title for the block's top border (ratatui draws
/// right-aligned titles after left-aligned ones, so a long path gives way, not the position).
pub fn scroll_title(e: ScrollExtent, w: ScrollWords) -> Line<'static> {
    Line::from(format!(" [{}] ", scroll_label(e, w))).right_aligned()
}

/// `ScrollbarState` of an extent. `content_length` is the number of scroll positions (`max + 1`)
/// and the viewport is explicit, which puts the thumb's bottom edge on the track's last row at the
/// last position. The triple is scaled down first (a windowed reader measures in bytes).
pub fn scrollbar_state(e: ScrollExtent) -> ScrollbarState {
    const CAP: u64 = 1 << 20;
    let span = e.max.saturating_add(e.viewport).max(1);
    let d = span.div_ceil(CAP).max(1);
    let (pos, max, viewport) = (e.pos / d, e.max / d, e.viewport / d);
    ScrollbarState::new((max + 1) as usize)
        .position(pos as usize)
        .viewport_content_length(viewport.max(1) as usize)
}

/// Draw the vertical thumb **over the block's right border column** (no text column is used).
/// `area` is the block's outer rect; the margin of 1 keeps the thumb off the corners. The track
/// and the arrow heads are off: the border is the track, and the arrows are East Asian Ambiguous.
pub fn render_scrollbar(buf: &mut Buffer, area: Rect, e: ScrollExtent) {
    if area.height < 3 || area.width == 0 {
        return;
    }
    let mut state = scrollbar_state(e);
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .track_symbol(None)
        .begin_symbol(None)
        .end_symbol(None)
        .render(
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            buf,
            &mut state,
        );
}
