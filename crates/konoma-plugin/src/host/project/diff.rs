//! Immutable diff data and the viewport layout cache shared by project hosts.

use ratatui::text::Line;
use std::sync::Arc;

/// Immutable diff data owns its layout cache, so a replacement view always invalidates it.
pub struct ProjectDiff {
    pub lines: Vec<konoma_vcs_core::git::DiffLine>,
    pub header: Vec<Line<'static>>,
    /// Drawn beside the project tabs, like a list's title.
    pub title: Option<Line<'static>>,
    pub theme: String,
    pub no_changes: String,
    split_rows: usize,
    unified_width: usize,
    cache: std::sync::Mutex<Option<Arc<ProjectDiffLayout>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    layouts: std::sync::atomic::AtomicUsize,
}

pub struct ProjectDiffLayout {
    pub lines: Vec<Line<'static>>,
    hscroll: u16,
    width: u16,
    split: bool,
    start: usize,
    count: usize,
}

impl ProjectDiff {
    pub fn new(
        lines: Vec<konoma_vcs_core::git::DiffLine>,
        header: Vec<Line<'static>>,
        theme: String,
        no_changes: String,
    ) -> Self {
        // This constructor runs on the project worker. Keep totals separate from highlighted
        // rows so key handling and row queries never need to render the document.
        let split_rows = konoma_widgets::gitdiff::side_by_side_row_count(&lines, "");
        let unified_width = konoma_widgets::gitdiff::unified_max_hscroll(&lines, 0)
            .max(header.iter().map(Line::width).max().unwrap_or(0));
        Self {
            lines,
            header,
            title: None,
            theme,
            no_changes,
            split_rows,
            unified_width,
            cache: Default::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            layouts: Default::default(),
        }
    }

    pub fn with_title(mut self, title: Option<Line<'static>>) -> Self {
        self.title = title;
        self
    }

    fn header_rows(&self) -> usize {
        self.header.len() + usize::from(!self.header.is_empty())
    }

    pub fn row_count(&self, split: bool) -> usize {
        self.header_rows()
            + if self.lines.is_empty() {
                usize::from(!self.header.is_empty())
            } else if split {
                self.split_rows
            } else {
                self.lines.len()
            }
    }

    pub fn metrics(&self, width: u16, split: bool) -> (usize, u16) {
        let max_h = if self.lines.is_empty() {
            0
        } else if split {
            konoma_widgets::gitdiff::side_by_side_max_hscroll(&self.lines, usize::from(width))
        } else {
            self.unified_width
                .max(usize::from(!self.header.is_empty()))
                .saturating_sub(usize::from(width))
        };
        (self.row_count(split), max_h.min(u16::MAX as usize) as u16)
    }

    pub fn layout(
        &self,
        width: u16,
        split: bool,
        offset: u16,
        start: usize,
        count: usize,
    ) -> Arc<ProjectDiffLayout> {
        use konoma_widgets::gitdiff::{diff_lines_range, diff_lines_side_by_side_range};
        use ratatui::style::{Color, Style};
        // Stacked panning only moves the Paragraph; it does not change its highlighted lines.
        let offset = if split { offset } else { 0 };
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(cached) = cache.as_ref().filter(|c| {
            c.width == width
                && c.split == split
                && c.hscroll == offset
                && c.start == start
                && c.count == count
        }) {
            return cached.clone();
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.layouts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let iw = usize::from(width);
        let end = start.saturating_add(count);
        let mut lines: Vec<_> = self
            .header
            .iter()
            .skip(start)
            .take(count)
            .cloned()
            .collect();
        if !self.header.is_empty() && start <= self.header.len() && self.header.len() < end {
            lines.push(Line::styled(
                "─".repeat(iw.max(1)),
                Style::new().fg(Color::DarkGray),
            ));
        }
        let header_rows = self.header_rows();
        let diff_start = start.saturating_sub(header_rows);
        let diff_count = end.saturating_sub(header_rows.max(start));
        if self.lines.is_empty() {
            if !self.header.is_empty() && start <= header_rows && header_rows < end {
                lines.push(Line::styled(
                    self.no_changes.clone(),
                    Style::new().fg(Color::DarkGray),
                ));
            }
        } else if diff_count > 0 {
            if split {
                lines.extend(diff_lines_side_by_side_range(
                    &self.lines,
                    "",
                    &self.theme,
                    iw,
                    usize::from(offset),
                    diff_start,
                    diff_count,
                ));
            } else {
                lines.extend(diff_lines_range(
                    &self.lines,
                    "",
                    &self.theme,
                    iw,
                    diff_start,
                    diff_count,
                ));
            }
        }
        let drawn = Arc::new(ProjectDiffLayout {
            lines,
            hscroll: offset,
            width,
            split,
            start,
            count,
        });
        *cache = Some(drawn.clone());
        drawn
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn layout_count(&self) -> usize {
        self.layouts.load(std::sync::atomic::Ordering::Relaxed)
    }
}
