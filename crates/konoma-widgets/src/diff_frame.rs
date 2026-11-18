//! Value-based frames for source diffs and decorated documents. Preparation and I/O belong to hosts.
use crate::gitdiff;
use crate::scroll::{render_scrollbar, scroll_title, ScrollExtent, ScrollWords};
use konoma_vcs_core::git::DiffLine;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget, Wrap},
};

/// Unresolved source layout. The adapter supplies the shared configuration's Auto threshold.
#[derive(Clone, Copy, Debug)]
pub enum SourceDiffLayout {
    Unified,
    Split,
    Auto { min_width: u16 },
}

impl SourceDiffLayout {
    pub fn is_split(self, inner_width: u16) -> bool {
        match self {
            Self::Unified => false,
            Self::Split => true,
            Self::Auto { min_width } => inner_width >= min_width,
        }
    }
}

/// Resolved path and localized scope; no host paths, actions or preview kinds.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiffTitleValues<'a> {
    pub path: Option<&'a str>,
    pub position: Option<(usize, usize)>,
    pub scope: Option<&'a str>,
    /// `None` for Source, `Some("rendered")` for the decorated diff.
    pub representation: Option<&'a str>,
}

impl DiffTitleValues<'_> {
    pub fn title(self, split: bool) -> String {
        let Some(path) = self.path else {
            return " diff ".to_string();
        };
        let tag = match self.representation {
            Some(name) => format!(" ⟨{name}⟩"),
            None if split => " ⇆".to_string(),
            None => String::new(),
        };
        let pos = self
            .position
            .map(|(i, n)| format!(" ({i}/{n})"))
            .unwrap_or_default();
        let scope = self.scope.map(|s| format!(" · {s}")).unwrap_or_default();
        format!(" diff{tag}: {path}{pos}{scope} ")
    }
}

pub struct SourceDiffValues<'a> {
    pub lines: &'a [DiffLine],
    pub extension: &'a str,
    pub theme: &'a str,
    pub layout: SourceDiffLayout,
    pub scroll: u16,
    pub hscroll: u16,
    pub title: DiffTitleValues<'a>,
    pub empty_text: &'a str,
    pub scroll_words: ScrollWords,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffDrawUpdate {
    pub scroll: u16,
    pub hscroll: u16,
    pub diff_split_drawn: bool,
}

/// Draw only the visible source window. Split panning moves each body, keeping its gutter fixed.
pub fn draw_source_diff(
    buf: &mut Buffer,
    area: Rect,
    values: SourceDiffValues<'_>,
) -> DiffDrawUpdate {
    let inner = Block::bordered().inner(area);
    let split = values.layout.is_split(inner.width);
    let mut update = DiffDrawUpdate {
        scroll: values.scroll,
        hscroll: values.hscroll,
        diff_split_drawn: split,
    };
    let titled = |extent| {
        Block::bordered()
            .title(values.title.title(split))
            .title(scroll_title(extent, values.scroll_words))
    };
    if values.lines.is_empty() {
        let extent = ScrollExtent::new(0, 0, inner.height as u64);
        titled(extent).render(area, buf);
        Paragraph::new(values.empty_text)
            .alignment(Alignment::Center)
            .render(
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height / 2,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
        render_scrollbar(buf, area, extent);
        return update;
    }
    let iw = inner.width as usize;
    let ih = inner.height as usize;
    let (lines, para_hscroll, max_v) = if split {
        let max_h = gitdiff::side_by_side_max_hscroll(values.lines, iw) as u16;
        update.hscroll = update.hscroll.min(max_h);
        let total_rows = gitdiff::side_by_side_row_count(values.lines, values.extension);
        let max_v = total_rows.saturating_sub(ih) as u16;
        update.scroll = update.scroll.min(max_v);
        let lines = gitdiff::diff_lines_side_by_side_range(
            values.lines,
            values.extension,
            values.theme,
            iw,
            update.hscroll as usize,
            update.scroll as usize,
            ih,
        );
        (lines, 0, max_v)
    } else {
        let max_v = values.lines.len().saturating_sub(ih) as u16;
        update.scroll = update.scroll.min(max_v);
        let max_h = gitdiff::unified_max_hscroll(values.lines, iw) as u16;
        update.hscroll = update.hscroll.min(max_h);
        let lines = gitdiff::diff_lines_range(
            values.lines,
            values.extension,
            values.theme,
            iw,
            update.scroll as usize,
            ih,
        );
        (lines, update.hscroll, max_v)
    };
    let extent = ScrollExtent::new(update.scroll as u64, max_v as u64, ih as u64);
    Paragraph::new(Text::from(lines))
        .block(titled(extent))
        .scroll((0, para_hscroll))
        .render(area, buf);
    render_scrollbar(buf, area, extent);
    update
}

/// Scroll bounds after the host has laid out the document and resolved any slide limit.
pub struct DecoratedScrollValues {
    pub vertical_limit: usize,
    pub max_line_cols: usize,
    pub inner_width: u16,
    pub viewport: u16,
    pub scroll: u16,
    pub hscroll: u16,
    pub wrap: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DecoratedScrollUpdate {
    pub scroll: u16,
    pub hscroll: u16,
    pub extent: ScrollExtent,
}

/// Preserve the host's original casts and bounds, including the non-saturating horizontal cast.
pub fn clamp_decorated_scroll(values: DecoratedScrollValues) -> DecoratedScrollUpdate {
    let max_v = values.vertical_limit.min(u16::MAX as usize) as u16;
    let scroll = values.scroll.min(max_v);
    let max_h = if values.wrap {
        0
    } else {
        values
            .max_line_cols
            .saturating_sub(values.inner_width as usize) as u16
    };
    DecoratedScrollUpdate {
        scroll,
        hscroll: values.hscroll.min(max_h),
        extent: ScrollExtent::new(scroll as u64, max_v as u64, values.viewport as u64),
    }
}

pub struct DecoratedFrameValues<'a> {
    pub lines: Vec<Line<'static>>,
    pub local_scroll: u16,
    pub hscroll: u16,
    pub wrap: bool,
    pub extent: ScrollExtent,
    pub title: &'a str,
    pub scroll_words: ScrollWords,
}

/// Draw the prepared slice before the host invokes the shared inline picture overlay.
pub fn draw_decorated_frame(buf: &mut Buffer, area: Rect, values: DecoratedFrameValues<'_>) {
    let block = Block::bordered()
        .title(values.title.to_string())
        .title(scroll_title(values.extent, values.scroll_words));
    let mut para = Paragraph::new(Text::from(values.lines)).block(block);
    if values.wrap {
        para = para.wrap(Wrap { trim: false });
    }
    para.scroll((values.local_scroll, values.hscroll))
        .render(area, buf);
    render_scrollbar(buf, area, values.extent);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinarySummaryValues {
    pub same_bytes: bool,
    pub old_len: Option<u64>,
    pub new_len: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct BinarySummaryWords<'a> {
    pub binary_file: &'a str,
    pub none: &'a str,
    pub same_size_different_content: &'a str,
    pub byte_unit: &'a str,
}

pub fn binary_summary(
    values: BinarySummaryValues,
    words: BinarySummaryWords<'_>,
    size_formatter: impl Fn(u64) -> String,
) -> Option<String> {
    if values.same_bytes {
        return None;
    }
    let fmt = |n| match n {
        Some(b) => size_formatter(b),
        None => words.none.to_string(),
    };
    let delta = match (values.old_len, values.new_len) {
        (Some(o), Some(n)) if o == n => Some(words.same_size_different_content.to_string()),
        (Some(o), Some(n)) => Some(format_byte_delta(n as i64 - o as i64, words.byte_unit)),
        _ => None,
    };
    let mut line = format!(
        "{}: {} → {}",
        words.binary_file,
        fmt(values.old_len),
        fmt(values.new_len)
    );
    if let Some(delta) = delta {
        line.push_str(&format!(" ({delta})"));
    }
    Some(line)
}

pub fn format_byte_delta(delta: i64, byte_unit: &str) -> String {
    let sign = if delta >= 0 { '+' } else { '-' };
    format!(
        "{sign}{} {byte_unit}",
        thousands_separated(delta.unsigned_abs())
    )
}

pub fn thousands_separated(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}
