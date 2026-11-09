//! Pure change-block navigation. Hosts own file identity, search and selection.
use konoma_vcs_core::git::DiffLine;
use konoma_widgets::gitdiff;

/// First 1-based line of each contiguous run of marked source lines.
pub fn marked_line_block_starts<T>(marks: &std::collections::HashMap<u32, T>) -> Vec<u32> {
    let mut starts: Vec<_> = marks
        .keys()
        .copied()
        .filter(|&line| line <= 1 || !marks.contains_key(&(line - 1)))
        .collect();
    starts.sort_unstable();
    starts
}

/// Merge touching or overlapping logical-line marks into ascending block starts.
pub fn block_starts(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<usize> {
    ranges.sort_by_key(|r| r.start);
    let mut out = Vec::new();
    let mut end = 0;
    for r in ranges {
        if out.is_empty() || r.start > end {
            out.push(r.start);
        }
        end = end.max(r.end);
    }
    out
}

/// Block starts in display rows, and the last scroll position of that layout.
#[derive(Debug, Default)]
pub struct DiffBlocks {
    pub rows: Vec<usize>,
    pub max_v: usize,
}

/// A remembered block is valid only for the host's current file and matching scroll.
/// Keeping its identity distinguishes late blocks that clamp to the same scroll position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffBlockCursor {
    pub scroll: usize,
    pub block: Option<usize>,
}

impl DiffBlockCursor {
    pub fn current_block(self, blocks: &DiffBlocks) -> Option<usize> {
        if let Some(i) = self.block {
            if blocks
                .rows
                .get(i)
                .is_some_and(|&row| row.min(blocks.max_v) == self.scroll)
            {
                return Some(i);
            }
        }
        blocks.rows.iter().rposition(|&row| row <= self.scroll)
    }
}

/// Step by the sign of `dir`, wrapping. No blocks leaves the host's state untouched.
pub fn step_change_block(
    blocks: &DiffBlocks,
    cursor: DiffBlockCursor,
    dir: i32,
) -> Option<DiffBlockCursor> {
    let n = blocks.rows.len();
    if n == 0 {
        return None;
    }
    let known = cursor
        .current_block(blocks)
        .filter(|&i| blocks.rows[i].min(blocks.max_v) == cursor.scroll);
    let target = match known {
        Some(i) => (i as i64 + dir.signum() as i64).rem_euclid(n as i64) as usize,
        None if dir >= 0 => blocks
            .rows
            .iter()
            .position(|&r| r > cursor.scroll)
            .unwrap_or(0),
        None => blocks
            .rows
            .iter()
            .rposition(|&r| r < cursor.scroll)
            .unwrap_or(n - 1),
    };
    Some(DiffBlockCursor {
        scroll: blocks.rows[target].min(blocks.max_v),
        block: Some(target),
    })
}

/// Source diff block rows, including the split layout's removed/added row folding.
pub fn source_diff_blocks(
    lines: &[DiffLine],
    extension: &str,
    split: bool,
    viewport: usize,
) -> DiffBlocks {
    let total = if split {
        gitdiff::side_by_side_row_count(lines, extension)
    } else {
        lines.len()
    };
    DiffBlocks {
        rows: gitdiff::change_block_starts(lines)
            .into_iter()
            .map(|i| gitdiff::scroll_row_of_line(lines, i, split))
            .collect(),
        max_v: total.saturating_sub(viewport),
    }
}

/// The current source block's 1-based new-file line, including pure deletions.
pub fn source_diff_edit_line(
    lines: &[DiffLine],
    blocks: &DiffBlocks,
    cursor: DiffBlockCursor,
) -> Option<usize> {
    let starts = gitdiff::change_block_starts(lines);
    let pick = cursor.current_block(blocks).unwrap_or(0);
    gitdiff::block_new_line(lines, *starts.get(pick)?).map(|n| n as usize)
}
