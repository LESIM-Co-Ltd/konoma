//! Block-level alignment between two versions of a Markdown document — the pure layer both the
//! `rendered` and `preview` diff presentations (`docs/FEATURE-MD-RENDERED-DIFF.md` §1) are built on
//! top of. `render::render_doc_diff` (this module's sibling) is the one consumer of [`block_ops`]'s
//! output; this file itself never touches a `Writer`, a `Line`, or any rendering concern at all.
//!
//! ## Why blocks, not lines
//!
//! A Markdown line and a rendered screen row have no fixed correspondence once wrapping and inline
//! decoration are applied — `app.rs`'s own "Reflow means there is no exact scroll-row -> source-line
//! mapping" comment documents the identical fact for the *scroll* side of this problem. A line-level
//! diff would have nothing stable to point *at* once rendered: it could say "source line 42 changed"
//! but not which rendered row that became. A top-level [`model::Block`]'s extent, by contrast, is a
//! `pulldown-cmark`-reported byte range that survives rendering intact — every line inside it renders
//! to some contiguous run of screen rows, whatever that run's own length turns out to be — so a
//! per-block classification stays meaningful after decoration in a way a per-line one would not.
//!
//! ## Why the top-level list only
//!
//! Only `Doc::blocks` (the document's own top-level sequence) is compared — never a `List`'s own
//! items or a `Quote`'s own body. A single changed item inside a 30-item list therefore makes the
//! *whole* list one `Replace` pair today; a finer, recursive comparison is
//! `docs/FEATURE-MD-RENDERED-DIFF.md`'s own §6 stage 2 (container recursion), not built here. This is
//! a documented, known limitation (see the "known limitation" tests below, which pin today's coarse
//! behavior so a future stage-2 change shows up as an intentional test update, not a silent
//! regression) — not an oversight.

use std::ops::Range;

use similar::{capture_diff_slices, Algorithm, DiffOp};

use super::model::{Block, Doc};

/// One step of aligning `old`'s top-level blocks with `new`'s, in document order. `Equal`/`Insert`
/// name only a `new` index: an `Equal` block still renders from `new`'s own copy (not `old`'s) —
/// see `render::render_doc_diff`'s own doc comment for why resolving against whichever `Doc` a
/// caller can still act on matters (reference-style links, footnote numbering). `Delete` names only
/// an `old` index — there is no `new` counterpart to point at. `Replace` names both: the one shape
/// that draws *two* blocks, old then new (`render::DiffMark::Removed` then `render::DiffMark::
/// Modified`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockOp {
    Equal { new: usize },
    Insert { new: usize },
    Delete { old: usize },
    Replace { old: usize, new: usize },
}

/// The comparison key for one top-level block: its own source slice, trailing whitespace stripped.
/// Trailing-whitespace-only edits — a stray space added at a paragraph's own end, a file gaining or
/// losing its final trailing newline — must compare `Equal`, matching every other whitespace-
/// insensitive corner of konoma's own write-back paths (e.g. `model::code_body_text`'s own trailing-
/// newline strip). `trim_end` alone (not `trim`) — a block's *leading* bytes are never incidental
/// whitespace pulldown-cmark would have already stripped (a `Block::src` always starts at the
/// construct's own first real character: `#`, a list marker, a fence, ...), so trimming the front
/// too would risk treating a genuinely different leading character as insignificant.
fn block_key<'s>(src: &'s str, block: &Block) -> &'s str {
    src[block.src.clone()].trim_end()
}

/// Aligns `old`'s and `new`'s top-level blocks and classifies each side's position into one of the
/// four [`BlockOp`] shapes, via `similar`'s own sequence diff over each block's normalized source
/// text ([`block_key`]) — the same crate, and the same default algorithm (`Algorithm::Myers`),
/// `git::diff_contents` already uses for the line-level follow-baseline diff, so a Markdown file's
/// `rendered`/`preview` diff and its `source` fallback (`docs/FEATURE-MD-RENDERED-DIFF.md` §1) never
/// disagree about which alignment *is* "the" diff, only how it ends up drawn.
///
/// `capture_diff_slices` already merges an adjacent run of deletions immediately followed by
/// insertions into a single `DiffOp::Replace { old_len, new_len, .. }` (`similar`'s own `Compact` +
/// `Replace` diff hooks, unconditionally applied inside `capture_diff_slices_deadline`) — this
/// function only has to decide how to turn *that* into `BlockOp::Replace` pairs: greedily, old to
/// new, front to back (`old_index + i` paired with `new_index + i` for `i` in
/// `0..min(old_len, new_len)`), with whichever side is longer spilling out as plain `Delete`/`Insert`
/// for its own remaining entries. No smarter pairing (e.g. by textual similarity) is attempted —
/// `similar` offers no such signal within one `Replace` group, and the design doc does not ask for
/// one: moved blocks are explicitly out of scope ("移動したブロックは Delete＋Insert として出る（移動
/// 検出はしない＝仕様）").
///
/// `old`/`new` must be `Doc::parse`d from `old_src`/`new_src` respectively — every `BlockOp` index
/// this returns is only meaningful against that same pair.
pub(crate) fn block_ops(
    old: &Doc<'_>,
    new: &Doc<'_>,
    old_src: &str,
    new_src: &str,
) -> Vec<BlockOp> {
    let old_keys: Vec<&str> = old.blocks.iter().map(|b| block_key(old_src, b)).collect();
    let new_keys: Vec<&str> = new.blocks.iter().map(|b| block_key(new_src, b)).collect();
    let diff_ops = capture_diff_slices(Algorithm::Myers, &old_keys, &new_keys);

    let mut out = Vec::new();
    for op in diff_ops {
        match op {
            DiffOp::Equal { new_index, len, .. } => {
                for i in 0..len {
                    out.push(BlockOp::Equal { new: new_index + i });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    out.push(BlockOp::Delete { old: old_index + i });
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    out.push(BlockOp::Insert { new: new_index + i });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    out.push(BlockOp::Replace {
                        old: old_index + i,
                        new: new_index + i,
                    });
                }
                for i in paired..old_len {
                    out.push(BlockOp::Delete { old: old_index + i });
                }
                for i in paired..new_len {
                    out.push(BlockOp::Insert { new: new_index + i });
                }
            }
        }
    }
    out
}

/// Which of the three left-edge markers one [`preview_marks`] range gets — the same three-color
/// convention the code/text preview's own change gutter (`app::GutterMark`) uses (green added /
/// amber modified / red deleted), kept as its own tiny type here (rather than reusing `app::
/// GutterMark` or `render::DiffMark`) for the identical reason `render::DiffMark`'s own doc comment
/// gives: this module has no dependency on `crate::app`/`crate::git`, and `Deleted` here names a
/// different thing than either of those two enums' own third variant — `render::DiffMark::Removed`
/// draws the *old* block's own content (the `rendered` presentation actually shows what was
/// deleted); `PreviewMark::Deleted` marks one anchor row of the *new* document's own next-drawn
/// block (nothing deleted is ever shown at all in the `preview` presentation — see this type's own
/// call site, `preview_marks`, for exactly which row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewMark {
    /// An inserted block (`BlockOp::Insert`), covering its own whole row range.
    Added,
    /// The new half of a `BlockOp::Replace` pair, covering its own whole row range.
    Modified,
    /// A one-row anchor: the first row of whichever block draws immediately after one or more
    /// deleted blocks (the last block's own final row, if the deletion runs to the end of the
    /// document instead).
    Deleted,
}

/// Derives the `preview` presentation's own gutter marks (`docs/FEATURE-MD-RENDERED-DIFF.md` §1's
/// third row) from `ops` (`block_ops(old, new, ..)`) and `new_block_rows` — `new`'s own top-level
/// blocks' final row ranges, in the same order as `new.blocks` (`render::RenderOut::block_rows`,
/// from rendering `new` **alone**, the ordinary non-diff decorated render every other Markdown
/// preview already produces — the `preview` presentation draws nothing but `new`'s own current
/// content, so it needs no `render_doc_diff` pass of its own at all).
///
/// `Insert`/`Replace` mark their own `new` block's whole row range `Added`/`Modified`; `Equal`
/// contributes nothing (unchanged). A `Delete` (or the deleted half of a `Replace`, which this
/// function never receives — `Replace`'s own `old` half is not a `Delete`) has nothing to draw at
/// all in this presentation, so instead it leaves a one-row `Deleted` anchor at the very first row
/// of whichever block draws immediately after it (the next `Equal`/`Insert`/`Replace`'s own `new`
/// row range) — or, if the deletion runs all the way to the end of the document, the *last* row of
/// the block drawn immediately before it.
///
/// Returned ranges are **non-overlapping and sorted** by construction: this function resolves a
/// row that would otherwise carry two marks at once — the anchor row of a `Delete` immediately
/// followed by an `Insert`/`Replace` with no intervening `Equal`, i.e. no context line for the
/// anchor to land on undisturbed — by priority `Deleted` > `Modified` > `Added` (a vanished
/// neighbor is the rarer, more surprising fact, so it wins the one glyph column a row can show),
/// then coalesces adjacent same-kind rows into one range the same way `render_doc_diff`'s own
/// `marks` already do (each entry there is one block's whole extent, not one row at a time either).
pub(crate) fn preview_marks(
    ops: &[BlockOp],
    new_block_rows: &[Range<usize>],
) -> Vec<(Range<usize>, PreviewMark)> {
    // Per-row resolution, `Deleted` > `Modified` > `Added`, matching the priority the doc comment
    // above promises. A `BTreeMap` keeps the final coalescing pass below trivial (iteration is
    // already row-ascending).
    let mut by_row: std::collections::BTreeMap<usize, PreviewMark> =
        std::collections::BTreeMap::new();
    let mark_range = |by_row: &mut std::collections::BTreeMap<usize, PreviewMark>,
                      range: Range<usize>,
                      mark: PreviewMark| {
        for row in range {
            let slot = by_row.entry(row).or_insert(mark);
            if rank(mark) > rank(*slot) {
                *slot = mark;
            }
        }
    };
    fn rank(m: PreviewMark) -> u8 {
        match m {
            PreviewMark::Added => 0,
            PreviewMark::Modified => 1,
            PreviewMark::Deleted => 2,
        }
    }

    let mut pending_delete = false;
    let mut last_drawn_end: Option<usize> = None;
    for op in ops {
        match *op {
            BlockOp::Equal { new: n } => {
                if let Some(range) = new_block_rows.get(n).cloned() {
                    if pending_delete {
                        if let Some(row) = range.clone().next() {
                            mark_range(&mut by_row, row..row + 1, PreviewMark::Deleted);
                        }
                        pending_delete = false;
                    }
                    last_drawn_end = Some(range.end);
                }
            }
            BlockOp::Insert { new: n } => {
                if let Some(range) = new_block_rows.get(n).cloned() {
                    if pending_delete {
                        if let Some(row) = range.clone().next() {
                            mark_range(&mut by_row, row..row + 1, PreviewMark::Deleted);
                        }
                        pending_delete = false;
                    }
                    mark_range(&mut by_row, range.clone(), PreviewMark::Added);
                    last_drawn_end = Some(range.end);
                }
            }
            BlockOp::Replace { old: _, new: n } => {
                if let Some(range) = new_block_rows.get(n).cloned() {
                    if pending_delete {
                        if let Some(row) = range.clone().next() {
                            mark_range(&mut by_row, row..row + 1, PreviewMark::Deleted);
                        }
                        pending_delete = false;
                    }
                    mark_range(&mut by_row, range.clone(), PreviewMark::Modified);
                    last_drawn_end = Some(range.end);
                }
            }
            BlockOp::Delete { old: _ } => {
                pending_delete = true;
            }
        }
    }
    // The document's own last block(s) were deleted (deletion ran to EOF): anchor to the last row
    // actually drawn before it, rather than dropping the mark.
    if pending_delete {
        if let Some(end) = last_drawn_end {
            if end > 0 {
                let row = end - 1;
                mark_range(&mut by_row, row..row + 1, PreviewMark::Deleted);
            }
        }
    }

    // Coalesce adjacent rows carrying the same mark into one range.
    let mut out: Vec<(Range<usize>, PreviewMark)> = Vec::new();
    for (row, mark) in by_row {
        match out.last_mut() {
            Some((range, last_mark)) if range.end == row && *last_mark == mark => {
                range.end = row + 1;
            }
            _ => out.push((row..row + 1, mark)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(old_src: &str, new_src: &str) -> Vec<BlockOp> {
        let old = Doc::parse(old_src);
        let new = Doc::parse(new_src);
        block_ops(&old, &new, old_src, new_src)
    }

    #[test]
    fn identical_documents_are_all_equal() {
        let src = "# Title\n\nSome text.\n\n- a\n- b\n";
        let doc = Doc::parse(src);
        let got = block_ops(&doc, &doc, src, src);
        assert_eq!(doc.blocks.len(), got.len());
        for (i, op) in got.iter().enumerate() {
            assert_eq!(*op, BlockOp::Equal { new: i });
        }
    }

    #[test]
    fn both_empty_is_empty() {
        assert_eq!(ops("", ""), Vec::new());
    }

    #[test]
    fn empty_to_nonempty_is_all_insert() {
        let new_src = "# Title\n\nBody.\n";
        let got = ops("", new_src);
        let new = Doc::parse(new_src);
        assert_eq!(got.len(), new.blocks.len());
        for (i, op) in got.iter().enumerate() {
            assert_eq!(*op, BlockOp::Insert { new: i });
        }
    }

    #[test]
    fn nonempty_to_empty_is_all_delete() {
        let old_src = "# Title\n\nBody.\n";
        let got = ops(old_src, "");
        let old = Doc::parse(old_src);
        assert_eq!(got.len(), old.blocks.len());
        for (i, op) in got.iter().enumerate() {
            assert_eq!(*op, BlockOp::Delete { old: i });
        }
    }

    #[test]
    fn insertion_at_head() {
        let old_src = "para one.\n\npara two.\n";
        let new_src = "new first.\n\npara one.\n\npara two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Insert { new: 0 },
                BlockOp::Equal { new: 1 },
                BlockOp::Equal { new: 2 },
            ]
        );
    }

    #[test]
    fn insertion_at_tail() {
        let old_src = "para one.\n\npara two.\n";
        let new_src = "para one.\n\npara two.\n\nnew last.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Equal { new: 0 },
                BlockOp::Equal { new: 1 },
                BlockOp::Insert { new: 2 },
            ]
        );
    }

    #[test]
    fn insertion_in_the_middle() {
        let old_src = "para one.\n\npara two.\n";
        let new_src = "para one.\n\nnew middle.\n\npara two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Equal { new: 0 },
                BlockOp::Insert { new: 1 },
                BlockOp::Equal { new: 2 },
            ]
        );
    }

    #[test]
    fn deletion_at_head() {
        let old_src = "gone first.\n\npara one.\n\npara two.\n";
        let new_src = "para one.\n\npara two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Delete { old: 0 },
                BlockOp::Equal { new: 0 },
                BlockOp::Equal { new: 1 },
            ]
        );
    }

    #[test]
    fn deletion_at_tail() {
        let old_src = "para one.\n\npara two.\n\ngone last.\n";
        let new_src = "para one.\n\npara two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Equal { new: 0 },
                BlockOp::Equal { new: 1 },
                BlockOp::Delete { old: 2 },
            ]
        );
    }

    #[test]
    fn deletion_in_the_middle() {
        let old_src = "para one.\n\ngone middle.\n\npara two.\n";
        let new_src = "para one.\n\npara two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Equal { new: 0 },
                BlockOp::Delete { old: 1 },
                BlockOp::Equal { new: 1 },
            ]
        );
    }

    /// 2 `Delete` immediately followed by 1 `Insert`, within one `similar` `Replace` group, pairs
    /// front-to-back: the first old block pairs with the sole new block (`Replace`), the second old
    /// block has no new counterpart left to pair with and spills out as a plain `Delete`.
    #[test]
    fn two_deletes_one_insert_pairs_as_replace_then_delete() {
        let old_src = "old one.\n\nold two.\n";
        let new_src = "new one.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Replace { old: 0, new: 0 },
                BlockOp::Delete { old: 1 }
            ]
        );
    }

    /// The mirror of the above: 1 `Delete` followed by 2 `Insert`s pairs the one old block with the
    /// *first* new block, and the second new block spills out as a plain `Insert`.
    #[test]
    fn one_delete_two_inserts_pairs_as_replace_then_insert() {
        let old_src = "old one.\n";
        let new_src = "new one.\n\nnew two.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Replace { old: 0, new: 0 },
                BlockOp::Insert { new: 1 }
            ]
        );
    }

    #[test]
    fn trailing_whitespace_only_difference_is_equal() {
        // Same paragraph, but the new version has trailing spaces and no final newline.
        let old_src = "para one.\n\npara two.\n";
        let new_src = "para one.\n\npara two.   ";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![BlockOp::Equal { new: 0 }, BlockOp::Equal { new: 1 }]
        );
    }

    /// A one-word change inside a paragraph marks only that paragraph `Replace` — the paragraph
    /// before and after it stay `Equal`, confirming the alignment is not coarsened to "the whole
    /// document changed" by one small edit.
    #[test]
    fn single_word_change_in_a_paragraph_replaces_only_that_block() {
        let old_src = "before.\n\nthe old word is here.\n\nafter.\n";
        let new_src = "before.\n\nthe new word is here.\n\nafter.\n";
        let got = ops(old_src, new_src);
        assert_eq!(
            got,
            vec![
                BlockOp::Equal { new: 0 },
                BlockOp::Replace { old: 1, new: 1 },
                BlockOp::Equal { new: 2 },
            ]
        );
    }

    /// Known, documented stage-1 limitation (module doc comment, "Why the top-level list only"): a
    /// list is one top-level block, so a single changed item inside it replaces the *whole* list, not
    /// just that item. Pinned here so stage 2 (container recursion, §6) shows up as an intentional
    /// test update.
    #[test]
    fn single_list_item_change_replaces_the_whole_list() {
        let old_src = "- one\n- two\n- three\n";
        let new_src = "- one\n- TWO\n- three\n";
        let old = Doc::parse(old_src);
        let new = Doc::parse(new_src);
        assert_eq!(
            old.blocks.len(),
            1,
            "the whole list is a single top-level block"
        );
        assert_eq!(new.blocks.len(), 1);
        let got = block_ops(&old, &new, old_src, new_src);
        assert_eq!(got, vec![BlockOp::Replace { old: 0, new: 0 }]);
    }

    /// No move detection (module/function doc comments): swapping a heading and a paragraph's own
    /// order renders as a plain delete-then-insert pair on each side, never a single "moved" op —
    /// `similar` has no such concept and this layer does not add one.
    #[test]
    fn reordering_two_blocks_is_delete_and_insert_not_a_move() {
        let old_src = "# Heading\n\nParagraph.\n";
        let new_src = "Paragraph.\n\n# Heading\n";
        let got = ops(old_src, new_src);
        // `BlockOp` has no "moved" variant at all — structurally, a reorder can only ever come out as
        // some combination of `Equal`/`Delete`/`Insert`/`Replace`, never a single op naming an old
        // position and a *different* new position for the identical content. What is worth pinning at
        // runtime is that `similar`'s LCS-based alignment (like plain `diff`/`git diff`, which also do
        // not reorder-detect) finds the one block that *can* stay in a consistent relative order across
        // both a longest-common-subsequence view — trivially one of the two elements here, since with
        // only two swapped elements no relative order survives for both at once — and reports it
        // `Equal`, while the other one, which cannot, comes out as a `Delete`+`Insert` pair rather than
        // a `Replace` (the two blocks' own byte content is never equal to each other, so nothing here
        // could ever legitimately pair as one `Replace`).
        let equal_count = got
            .iter()
            .filter(|op| matches!(op, BlockOp::Equal { .. }))
            .count();
        let delete_count = got
            .iter()
            .filter(|op| matches!(op, BlockOp::Delete { .. }))
            .count();
        let insert_count = got
            .iter()
            .filter(|op| matches!(op, BlockOp::Insert { .. }))
            .count();
        let replace_count = got
            .iter()
            .filter(|op| matches!(op, BlockOp::Replace { .. }))
            .count();
        assert_eq!(
            (equal_count, delete_count, insert_count, replace_count),
            (1, 1, 1, 0),
            "a two-block reorder must land as exactly one Equal (whichever block the LCS keeps) plus \
             one Delete and one Insert (the other block, moved) — never a Replace pairing two \
             genuinely different blocks' content, and never fewer/more ops than that: {got:?}"
        );
    }

    /// Invariant: for any pair of documents, walking `ops` covers `new`'s own block indices
    /// `0..new.blocks.len()` in strictly ascending order, exactly once each, and `old`'s own indices
    /// `0..old.blocks.len()` the same way — every position on both sides is accounted for by exactly
    /// one op, never zero, never two. Checked over 30 pseudo-randomly generated document pairs (a
    /// tiny in-test xorshift PRNG — no `rand` dependency — seeded per case for reproducibility).
    #[test]
    fn every_block_on_both_sides_is_covered_exactly_once_in_order() {
        for seed in 0..30u64 {
            let mut rng = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
            let old_src = random_doc_src(&mut rng);
            let new_src = random_doc_src(&mut rng);
            let old = Doc::parse(&old_src);
            let new = Doc::parse(&new_src);
            let got = block_ops(&old, &new, &old_src, &new_src);

            let mut next_new = 0usize;
            let mut next_old = 0usize;
            for op in &got {
                match *op {
                    // `Equal` names only a `new` index (`BlockOp`'s own doc comment), but still
                    // consumes exactly one `old` block too — a matched pair, one from each side, in
                    // lockstep — so `next_old` must advance here as well, even with nothing to assert
                    // it against.
                    BlockOp::Equal { new: n } => {
                        assert_eq!(
                            n, next_new,
                            "seed {seed}: new index out of order, ops: {got:?}, old_src: {old_src:?}, new_src: {new_src:?}"
                        );
                        next_new += 1;
                        next_old += 1;
                    }
                    BlockOp::Insert { new: n } => {
                        assert_eq!(
                            n, next_new,
                            "seed {seed}: new index out of order, ops: {got:?}, old_src: {old_src:?}, new_src: {new_src:?}"
                        );
                        next_new += 1;
                    }
                    BlockOp::Delete { old: o } => {
                        assert_eq!(
                            o, next_old,
                            "seed {seed}: old index out of order, ops: {got:?}"
                        );
                        next_old += 1;
                    }
                    BlockOp::Replace { old: o, new: n } => {
                        assert_eq!(o, next_old, "seed {seed}: replace old index out of order");
                        assert_eq!(n, next_new, "seed {seed}: replace new index out of order");
                        next_old += 1;
                        next_new += 1;
                    }
                }
            }
            assert_eq!(
                next_old,
                old.blocks.len(),
                "seed {seed}: did not cover every old block"
            );
            assert_eq!(
                next_new,
                new.blocks.len(),
                "seed {seed}: did not cover every new block"
            );
        }
    }

    /// Tiny xorshift64* PRNG — deterministic, dependency-free, good enough to shake out edge cases in
    /// [`every_block_on_both_sides_is_covered_exactly_once_in_order`] without pulling in `rand`.
    fn next_rand(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// Builds a pseudo-random small Markdown document out of a fixed pool of top-level block shapes
    /// (heading, paragraph, list, code fence, blockquote, thematic break, empty) — varied count and
    /// content so the corpus exercises `Equal`/`Insert`/`Delete`/`Replace` in combination, not just
    /// the hand-picked single-edit cases above.
    fn random_doc_src(state: &mut u64) -> String {
        let block_count = (next_rand(state) % 6) as usize; // 0..=5 top-level blocks
        let mut out = String::new();
        for i in 0..block_count {
            let choice = next_rand(state) % 6;
            let variant = next_rand(state) % 5;
            match choice {
                0 => out.push_str(&format!("# Heading {i} v{variant}\n\n")),
                1 => out.push_str(&format!("Paragraph {i} content v{variant}.\n\n")),
                2 => out.push_str(&format!("- item {i}a v{variant}\n- item {i}b\n\n")),
                3 => out.push_str(&format!("```\ncode {i} v{variant}\n```\n\n")),
                4 => out.push_str(&format!("> quoted {i} v{variant}\n\n")),
                _ => out.push_str("---\n\n"),
            }
        }
        out
    }

    // --- preview_marks -----------------------------------------------------------------------

    /// Fabricated `new_block_rows`: block `i` occupies rows `[sum of previous heights, +heights[i])`.
    /// Real callers get this from `render::RenderOut::block_rows`; these tests only need *some*
    /// non-overlapping, ascending row ranges to check `preview_marks`'s own row-level logic against
    /// — the heights themselves don't have to mean anything.
    fn rows(heights: &[usize]) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for h in heights {
            out.push(at..at + h);
            at += h;
        }
        out
    }

    #[test]
    fn all_equal_has_no_marks() {
        let ops = vec![
            BlockOp::Equal { new: 0 },
            BlockOp::Equal { new: 1 },
            BlockOp::Equal { new: 2 },
        ];
        assert_eq!(preview_marks(&ops, &rows(&[2, 3, 1])), Vec::new());
    }

    #[test]
    fn insert_marks_its_own_whole_range_added() {
        let ops = vec![BlockOp::Equal { new: 0 }, BlockOp::Insert { new: 1 }];
        let got = preview_marks(&ops, &rows(&[2, 3]));
        assert_eq!(got, vec![(2..5, PreviewMark::Added)]);
    }

    #[test]
    fn replace_marks_its_own_whole_range_modified() {
        let ops = vec![
            BlockOp::Equal { new: 0 },
            BlockOp::Replace { old: 5, new: 1 },
        ];
        let got = preview_marks(&ops, &rows(&[2, 4]));
        assert_eq!(got, vec![(2..6, PreviewMark::Modified)]);
    }

    /// A lone `Delete` followed by an `Equal` anchors a single-row `Deleted` mark on the `Equal`
    /// block's own first row (not its whole range — only the code/text gutter's boundary marker
    /// shape, one row, not a whole block).
    #[test]
    fn delete_then_equal_anchors_first_row_of_the_equal_block() {
        let ops = vec![BlockOp::Delete { old: 0 }, BlockOp::Equal { new: 0 }];
        let got = preview_marks(&ops, &rows(&[3]));
        assert_eq!(got, vec![(0..1, PreviewMark::Deleted)]);
    }

    /// A deletion running all the way to the end of the document (nothing drawn after it) anchors
    /// to the *last* row of the block drawn immediately *before* it instead.
    #[test]
    fn delete_at_end_of_document_anchors_last_row_of_the_previous_block() {
        let ops = vec![BlockOp::Equal { new: 0 }, BlockOp::Delete { old: 1 }];
        let got = preview_marks(&ops, &rows(&[4]));
        assert_eq!(got, vec![(3..4, PreviewMark::Deleted)]);
    }

    /// A document whose *every* block was deleted (new is empty) leaves no anchor at all — there is
    /// no row left in the new document to put one on.
    #[test]
    fn delete_of_the_whole_document_leaves_no_anchor() {
        let ops = vec![BlockOp::Delete { old: 0 }, BlockOp::Delete { old: 1 }];
        assert_eq!(preview_marks(&ops, &[]), Vec::new());
    }

    /// Two consecutive deletions collapse into the same one-row anchor as a single deletion would
    /// (the anchor names *a position*, not *how many* blocks vanished there).
    #[test]
    fn two_consecutive_deletes_still_anchor_one_row() {
        let ops = vec![
            BlockOp::Delete { old: 0 },
            BlockOp::Delete { old: 1 },
            BlockOp::Equal { new: 0 },
        ];
        let got = preview_marks(&ops, &rows(&[2]));
        assert_eq!(got, vec![(0..1, PreviewMark::Deleted)]);
    }

    /// When a `Delete` is immediately followed by an `Insert`/`Replace` (no intervening `Equal` —
    /// the pairing `block_ops` itself prefers whenever it can, so this is the *common* shape, not a
    /// rare corner), the anchor row and the inserted/modified block's own first row are the same
    /// row. Priority (doc comment: `Deleted` > `Modified` > `Added`) resolves the conflict: the
    /// vanished-neighbor signal wins that one row, and the rest of the inserted/modified range keeps
    /// its own color.
    #[test]
    fn delete_immediately_before_insert_prioritizes_deleted_on_the_shared_row() {
        let ops = vec![BlockOp::Delete { old: 0 }, BlockOp::Insert { new: 0 }];
        let got = preview_marks(&ops, &rows(&[3]));
        assert_eq!(
            got,
            vec![(0..1, PreviewMark::Deleted), (1..3, PreviewMark::Added)]
        );
    }

    #[test]
    fn delete_immediately_before_replace_prioritizes_deleted_on_the_shared_row() {
        let ops = vec![
            BlockOp::Delete { old: 0 },
            BlockOp::Replace { old: 1, new: 0 },
        ];
        let got = preview_marks(&ops, &rows(&[2]));
        assert_eq!(
            got,
            vec![(0..1, PreviewMark::Deleted), (1..2, PreviewMark::Modified)]
        );
    }

    /// Ranges out never overlap and are always sorted ascending, over the same randomized corpus
    /// `every_block_on_both_sides_is_covered_exactly_once_in_order` already exercises (reusing the
    /// same generator keeps this a true property check rather than a handful of hand-picked cases).
    #[test]
    fn marks_are_always_sorted_and_non_overlapping() {
        for seed in 0..30u64 {
            let mut rng = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(7);
            let old_src = random_doc_src(&mut rng);
            let new_src = random_doc_src(&mut rng);
            let old = Doc::parse(&old_src);
            let new = Doc::parse(&new_src);
            let ops = block_ops(&old, &new, &old_src, &new_src);
            // One fabricated row per block (heights of 1) is enough to check ordering/overlap —
            // the exact row *count* per block is render.rs's own concern, exercised separately by
            // `render::tests` against real `block_rows`.
            let block_rows = rows(&vec![1; new.blocks.len()]);
            let got = preview_marks(&ops, &block_rows);
            for w in got.windows(2) {
                assert!(
                    w[0].0.end <= w[1].0.start,
                    "seed {seed}: overlapping/out-of-order marks: {got:?}"
                );
            }
        }
    }
}
