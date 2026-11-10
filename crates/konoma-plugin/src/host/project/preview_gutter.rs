//! Pure change gutters for file previews. No baseline reads, cache ownership or alignment work.

use super::md_diff::MdDiffOutcome;
use konoma_vcs_core::git::DiffLine;
use konoma_widgets::markdown::{self, BlockOp, PreviewMark};
use ratatui::text::{Line, Span};
use std::ops::Range;

/// Initial position around a change, in zero-based file lines or display rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstChangePosition {
    pub top: usize,
    pub caret: usize,
}

/// Keep three lines of context above the first change. Marks and baseline selection belong
/// to the caller; use a rendered display row for a decorated document.
pub fn first_change_position(first: usize) -> FirstChangePosition {
    FirstChangePosition {
        top: first.saturating_sub(3),
        caret: first,
    }
}

/// Editor-style git change status of one line in the previewed (working-tree) file, for the left gutter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GutterMark {
    /// Newly inserted line (no matching removal): green bar.
    Added,
    /// Line replaced/edited (part of a hunk that also removed lines): amber bar.
    Modified,
    /// One or more lines were removed just above this line: red boundary marker.
    Deleted,
}

// Gutter marker colors, matching Zed: green added, amber modified, red deleted.
use super::rendered_diff::{GUTTER_ADDED, GUTTER_DELETED, GUTTER_MODIFIED};

/// Derive a per-new-line git gutter map from a file's working-tree diff. Keyed by 1-based new-file line
/// number. A change block is a maximal run of Added/Removed lines (Context breaks it): with additions it
/// marks each added line Modified (if the block also removed lines) or Added (pure insertion); a
/// pure-deletion block marks the following line Deleted (or the preceding line if the deletion is at EOF).
pub fn line_gutter_marks(diff: &[DiffLine]) -> std::collections::HashMap<u32, GutterMark> {
    use konoma_vcs_core::git::DiffLineKind;
    let mut marks = std::collections::HashMap::new();
    let mut i = 0;
    while i < diff.len() {
        if diff[i].kind == DiffLineKind::Context {
            i += 1;
            continue;
        }
        let start = i;
        let mut removed = 0usize;
        let mut added: Vec<u32> = Vec::new();
        while i < diff.len() && diff[i].kind != DiffLineKind::Context {
            match diff[i].kind {
                DiffLineKind::Removed => removed += 1,
                DiffLineKind::Added => {
                    if let Some(n) = diff[i].new_no {
                        added.push(n);
                    }
                }
                DiffLineKind::Context => {}
            }
            i += 1;
        }
        if !added.is_empty() {
            let mark = if removed > 0 {
                GutterMark::Modified
            } else {
                GutterMark::Added
            };
            for n in added {
                marks.insert(n, mark);
            }
        } else if removed > 0 {
            // Pure deletion: anchor the marker to the line just below the removed block; if the block is
            // at EOF, anchor to the last real line above it (deletion below).
            let anchor = diff
                .get(i)
                .and_then(|d| d.new_no)
                .or_else(|| diff[..start].iter().rev().find_map(|d| d.new_no));
            if let Some(a) = anchor {
                marks.entry(a).or_insert(GutterMark::Deleted);
            }
        }
    }
    marks
}

/// Prepend a one-cell git change marker to each line (before the line-number gutter, Zed-style). Only
/// applied when the file has changes (`marks` non-empty) — unchanged files / non-repos keep their layout.
/// The displayed line `i` maps to 1-based file line `top_line + i + 1`; unchanged lines get a blank cell.
pub fn with_line_gutter(
    lines: Vec<Line<'static>>,
    top_line: usize,
    marks: &std::collections::HashMap<u32, GutterMark>,
) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Style};
    if marks.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let n = (top_line + i + 1) as u32;
            let (glyph, color) = match marks.get(&n) {
                Some(GutterMark::Added) => ("▌", GUTTER_ADDED),
                Some(GutterMark::Modified) => ("▌", GUTTER_MODIFIED),
                // Thin bar hugging the TOP edge of the line below a deletion, so it
                // sits on the seam between the two lines instead of looking like the
                // line itself was removed. (A terminal cell can't draw in the inter-row
                // gap, so the top edge is the closest we can get without adding a row.)
                Some(GutterMark::Deleted) => ("▔", GUTTER_DELETED),
                None => (" ", Color::Reset),
            };
            let style = line.style;
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(glyph, Style::new().fg(color)));
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// Prepend the ordinary Markdown preview's boundary markers to a visible logical-row slice.
/// Deleted rows are anchors in the current content; no body span is dimmed.
pub fn with_preview_gutter(
    lines: Vec<Line<'static>>,
    lo: usize,
    marks: &[(Range<usize>, PreviewMark)],
) -> Vec<Line<'static>> {
    use konoma_widgets::markdown::PreviewMark;
    use ratatui::style::{Color, Style};
    if marks.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let row = lo + i;
            let mark = marks
                .iter()
                .find(|(range, _)| range.contains(&row))
                .map(|(_, m)| *m);
            let (glyph, color) = match mark {
                Some(PreviewMark::Added) => ("▌", GUTTER_ADDED),
                Some(PreviewMark::Modified) => ("▌", GUTTER_MODIFIED),
                Some(PreviewMark::Deleted) => ("▔", GUTTER_DELETED),
                None => (" ", Color::Reset),
            };
            let style = line.style;
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(glyph, Style::new().fg(color)));
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// Borrow the worker's landed alignment for an ordinary Markdown file preview.
/// Activity is decided before rendering; projection uses that render's own block rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePreviewGutter<'a> {
    ops: &'a [BlockOp],
}

impl<'a> FilePreviewGutter<'a> {
    pub fn new(ops: &'a [BlockOp]) -> Self {
        Self { ops }
    }

    pub fn from_outcome(outcome: Option<&'a MdDiffOutcome>) -> Option<Self> {
        match outcome {
            Some(MdDiffOutcome::Ready { ops, .. }) => Some(Self::new(ops)),
            _ => None,
        }
    }

    pub fn has_changes(self) -> bool {
        markdown::ops_has_any_change(self.ops)
    }

    /// The landed operations, for a host that retains its own alignment cache.
    pub fn ops(self) -> &'a [BlockOp] {
        self.ops
    }

    pub fn project(self, block_rows: &[Range<usize>]) -> Vec<(Range<usize>, PreviewMark)> {
        markdown::preview_marks(self.ops, block_rows)
    }
}
