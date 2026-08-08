// Line generation for the git diff preview (unified, Zed-style coloring).
//
// Spec (DIFF VISUAL SPEC):
//   - Each line has a kind: Context (no background) / Added (green background) / Removed (red background).
//   - Gutter: **a 1-column change bar "▌" at the front** (left of the line numbers) + the old line
//     number (right-aligned, dim) + the new line number (dim).
//     Bar color: Added=green / Removed=red / Context=blank. No literal +/- is shown.
//   - Body: syntax-colored by syntect from the extension, with the diff's background color
//     overlaid on top (foreground=syntect, background=diff, across the whole line). The background
//     is dark enough to stay visible (the bright foreground remains readable).
//
// side-by-side is implemented in `diff_lines_side_by_side` (`s` toggles stacked ⇄ side-by-side).
// The changed characters within a line are found via intra_ranges, and only those characters get a
// slightly brighter background (word-level emphasis).
// Hunk-level staging is DEFERRED.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::git::{DiffLine, DiffLineKind};

/// Frame color for file boundaries (a neutral blue that does not blend with the red/green of the body).
const HEADER_FG: Color = Color::Rgb(122, 162, 247);

/// Whether this DiffLine is a file boundary header (Context with neither old nor new line number).
/// A normal Context line always has both line numbers, so both being None means it is a header.
fn is_file_header(dl: &DiffLine) -> bool {
    matches!(dl.kind, DiffLineKind::Context) && dl.old_no.is_none() && dl.new_no.is_none()
}

/// Extract the file extension from a header path (empty if none → plain display).
fn ext_from_path(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

/// Build a **framed header line** for a file boundary. `┌─ <path> ─…─┐` (full width, blue, square corners). Replaces the `── path ──` decoration.
fn file_header_line(path: &str, width: usize) -> Line<'static> {
    let line_style = Style::new().fg(HEADER_FG);
    let name_style = line_style.add_modifier(Modifier::BOLD);
    let label = format!(" {path} ");
    let label_w = UnicodeWidthStr::width(label.as_str());
    let used = 2 + label_w + 1; // "┌─" + label + "┐"
    let fill = width.saturating_sub(used);
    Line::from(vec![
        Span::styled("┌─".to_string(), line_style),
        Span::styled(label, name_style),
        Span::styled("─".repeat(fill), line_style),
        Span::styled("┐".to_string(), line_style),
    ])
}

/// Background for Added lines (dark green). Dark enough that syntect's bright foreground stays readable.
const BG_ADDED: Color = Color::Rgb(20, 48, 28);
/// Background for Removed lines (dark red).
const BG_REMOVED: Color = Color::Rgb(58, 24, 26);
/// Background for the **characters that actually changed** within a line (slightly brighter than the line background). For word-level emphasis.
const BG_ADDED_STRONG: Color = Color::Rgb(40, 92, 54);
const BG_REMOVED_STRONG: Color = Color::Rgb(104, 40, 46);
/// Change bar color.
const BAR_ADDED: Color = Color::Rgb(87, 171, 90);
const BAR_REMOVED: Color = Color::Rgb(199, 84, 80);
/// Width of the gutter numbers (per column). Line numbers exceeding this do not wrap (large files may overflow the digits).
const GUTTER_W: usize = 4;

/// From a paired old/new line, compute the **range of changed characters** ([start,end) in chars).
/// The middle left after stripping the common prefix and suffix is the changed part. e.g. `…= 1;` / `…= 2;` → only `1`/`2`.
/// If there is no common part at all (= entirely different lines), return an empty range and the caller adds no emphasis.
fn word_change_range(old: &str, new: &str) -> ((usize, usize), (usize, usize)) {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let cap = o.len().min(n.len());
    let mut p = 0;
    while p < cap && o[p] == n[p] {
        p += 1;
    }
    let mut s = 0;
    while s < cap - p && o[o.len() - 1 - s] == n[n.len() - 1 - s] {
        s += 1;
    }
    ((p, o.len() - s), (p, n.len() - s))
}

/// Return the "range of changed characters" for each DiffLine. Only lines where Removed/Added pair up (= edits) are Some.
/// Lines with no common prefix/suffix at all (entirely different) are not emphasized (None).
fn intra_ranges(diff: &[DiffLine]) -> Vec<Option<(usize, usize)>> {
    let mut ranges: Vec<Option<(usize, usize)>> = vec![None; diff.len()];
    let mut rem: Vec<usize> = Vec::new();
    let mut add: Vec<usize> = Vec::new();
    for (i, dl) in diff.iter().enumerate() {
        match dl.kind {
            DiffLineKind::Removed => {
                if !add.is_empty() {
                    pair_ranges(diff, &mut rem, &mut add, &mut ranges);
                }
                rem.push(i);
            }
            DiffLineKind::Added => add.push(i),
            DiffLineKind::Context => pair_ranges(diff, &mut rem, &mut add, &mut ranges),
        }
    }
    pair_ranges(diff, &mut rem, &mut add, &mut ranges);
    ranges
}

/// Pair up the accumulated deletions/additions top-down and write the change ranges into `ranges`.
fn pair_ranges(
    diff: &[DiffLine],
    rem: &mut Vec<usize>,
    add: &mut Vec<usize>,
    ranges: &mut [Option<(usize, usize)>],
) {
    let n = rem.len().min(add.len());
    for i in 0..n {
        let (oi, ni) = (rem[i], add[i]);
        let (ro, rn) = word_change_range(&diff[oi].text, &diff[ni].text);
        let o_len = diff[oi].text.chars().count();
        let n_len = diff[ni].text.chars().count();
        // If there's no common part at all (= a total replacement/entirely different), don't emphasize (the whole line stays the base color).
        let has_common = ro.0 > 0 || ro.1 < o_len || rn.0 > 0 || rn.1 < n_len;
        if has_common {
            ranges[oi] = Some(ro);
            ranges[ni] = Some(rn);
        }
    }
    rem.clear();
    add.clear();
}

/// Lay the line background `base` under the already-colored content spans while overlaying `strong` **only on the change range `range`**
/// (per char). Keep the foreground (syntect) and replace only the background. If `range`=None, apply `base` to the whole.
fn overlay_intra_bg(
    spans: Vec<Span<'static>>,
    base: Color,
    strong: Color,
    range: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    let (start, end) = match range {
        Some(r) => r,
        None => {
            // Lay `base` uniformly across it.
            return spans
                .into_iter()
                .map(|mut sp| {
                    sp.style = sp.style.bg(base);
                    sp
                })
                .collect();
        }
    };
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut idx = 0usize; // char index
    for sp in spans {
        let mut cur = String::new();
        let mut cur_strong = false;
        let mut started = false;
        for ch in sp.content.chars() {
            let is_strong = idx >= start && idx < end;
            if !started {
                cur_strong = is_strong;
                started = true;
            } else if is_strong != cur_strong {
                let bg = if cur_strong { strong } else { base };
                out.push(Span::styled(std::mem::take(&mut cur), sp.style.bg(bg)));
                cur_strong = is_strong;
            }
            cur.push(ch);
            idx += 1;
        }
        if !cur.is_empty() {
            let bg = if cur_strong { strong } else { base };
            out.push(Span::styled(cur, sp.style.bg(bg)));
        }
    }
    out
}

// Test-only counter of how many rows actually got syntax-highlighted (`highlight_line_by_ext`,
// called from `diff_line_to_line` for the unified layout and `render_half` for side-by-side).
// `highlight_line_by_ext` itself lives in `preview/code.rs` and can't be instrumented from here,
// but every call to it from this module goes through exactly these two spots, so counting at the
// call site measures the same thing: whether a render pass highlighted the whole diff or only the
// rows it actually drew. See `diff_lines_range`'s doc comment for why this matters (measured
// ~60µs/row — the dominant cost of rendering a large diff).
#[cfg(test)]
thread_local! {
    static HIGHLIGHT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_highlight_calls() {
    HIGHLIGHT_CALLS.with(|c| c.set(0));
}

#[cfg(test)]
fn highlight_calls() -> usize {
    HIGHLIGHT_CALLS.with(|c| c.get())
}

// `pub(crate)` re-exports of the two accessors above, for `e2e_tests.rs` (a sibling module, not a
// descendant of this one, so the module-private `fn`s above aren't reachable from it). Kept as
// thin wrappers rather than widening the originals' visibility so the unit tests in this file
// (which call the module-private names directly) don't have to change. Still `#[cfg(test)]`-gated
// — this thread-local counter must never exist in a release binary. Their only callers are the
// `#[cfg(feature = "git")]` e2e tests (opening a git diff needs the `git` feature), so under
// `--no-default-features` they'd otherwise be flagged dead code.
#[cfg(test)]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub(crate) fn reset_highlight_calls_for_test() {
    reset_highlight_calls();
}

#[cfg(test)]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub(crate) fn highlight_calls_for_test() -> usize {
    highlight_calls()
}

#[cfg(test)]
fn record_highlight_call() {
    HIGHLIGHT_CALLS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(test))]
fn record_highlight_call() {}

/// Turn a single DiffLine into a `Line` of gutter + change bar + colored body.
/// `ext`/`theme` are for syntect coloring of the body. The background is overlaid on all spans by line kind.
pub fn diff_line_to_line(
    dl: &DiffLine,
    intra: Option<(usize, usize)>,
    ext: &str,
    theme: &str,
) -> Line<'static> {
    let (row_bg, strong_bg, bar_color, bar) = match dl.kind {
        DiffLineKind::Added => (Some(BG_ADDED), Some(BG_ADDED_STRONG), Some(BAR_ADDED), "▌"),
        DiffLineKind::Removed => (
            Some(BG_REMOVED),
            Some(BG_REMOVED_STRONG),
            Some(BAR_REMOVED),
            "▌",
        ),
        DiffLineKind::Context => (None, None, None, " "),
    };

    // Gutter: old line number (right-aligned) + new line number (right-aligned). The absent side is blank.
    let old = dl
        .old_no
        .map(|n| format!("{n:>GUTTER_W$}"))
        .unwrap_or_else(|| " ".repeat(GUTTER_W));
    let new = dl
        .new_no
        .map(|n| format!("{n:>GUTTER_W$}"))
        .unwrap_or_else(|| " ".repeat(GUTTER_W));

    let mut dim = Style::new().fg(Color::DarkGray);
    if let Some(bg) = row_bg {
        dim = dim.bg(bg);
    }

    // Place the change bar (1 column) **at the front = left of the line numbers** (color = line kind, background = line kind).
    let mut bar_style = Style::new();
    if let Some(c) = bar_color {
        bar_style = bar_style.fg(c);
    }
    if let Some(bg) = row_bg {
        bar_style = bar_style.bg(bg);
    }
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(bar.to_string(), bar_style),
        Span::styled(old, dim),
        Span::styled(" ", dim),
        Span::styled(new, dim),
        Span::styled(" ", dim),
    ];

    // Body: overlay the diff background on top of the syntax-colored (foreground). Only the changed characters get a slightly brighter background (intra).
    let content = if dl.text.is_empty() {
        vec![Span::raw(String::new())]
    } else {
        record_highlight_call();
        crate::preview::code::highlight_line_by_ext(&dl.text, ext, theme)
    };
    match (row_bg, strong_bg) {
        (Some(base), Some(strong)) => spans.extend(overlay_intra_bg(content, base, strong, intra)),
        _ => spans.extend(content), // Context: no background
    }

    Line::from(spans)
}

/// The file extension in effect at diff row `at` — i.e., from the nearest file-header row
/// strictly before `at`, or `default_ext` if there is none. Pure string/bool work (no
/// highlighting), so a linear scan back to the start of the diff on every render call is cheap
/// (measured ~50µs for 6,500 rows) next to the highlighting cost it lets `diff_lines_range` skip
/// for everything before the visible window.
fn ext_at(diff: &[DiffLine], default_ext: &str, at: usize) -> String {
    diff[..at.min(diff.len())]
        .iter()
        .rev()
        .find(|dl| is_file_header(dl))
        .map(|dl| ext_from_path(&dl.text))
        .unwrap_or_else(|| default_ext.to_string())
}

/// Render only the visible window `[start, start+count)` of the unified diff. A `DiffLine` always
/// maps to exactly one output row (file headers included — see
/// `unified_total_rows_is_diff_len_with_headers`), so `diff.len()` is already the total row count
/// and this is a plain index slice; unlike side-by-side, no separate layout pass is needed first
/// to find how many rows there are.
///
/// Syntax highlighting (`highlight_line_by_ext`, via `diff_line_to_line`) is the expensive part —
/// measured ~60µs/row, ~400–760ms for a real 6,500-row diff when it ran over the whole document on
/// every keypress (a keypress's render budget is ~60ms) — so it must run only for the rows
/// actually returned. `intra_ranges` is still computed over the **whole** diff regardless of the
/// window: the word-level highlight for a changed line depends on how many Removed/Added lines
/// precede it *within its change block*, and that block can start above `start` — a window that
/// starts mid-block would pair the wrong lines if `intra_ranges` only saw the slice (verified by
/// `diff_lines_range_preserves_intra_pairing_when_window_splits_a_change_block`, which fails if
/// this switches to slice-local pairing). That's fine: `intra_ranges` never highlights anything
/// (no syntect), measured ~110µs for 6,500 rows — negligible next to what it lets us skip.
pub fn diff_lines_range(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    let start = start.min(diff.len());
    let end = start.saturating_add(count).min(diff.len());
    if start >= end {
        return Vec::new();
    }
    let ranges = intra_ranges(diff);
    let mut cur_ext = ext_at(diff, default_ext, start);
    diff[start..end]
        .iter()
        .enumerate()
        .map(|(off, dl)| {
            let i = start + off;
            if is_file_header(dl) {
                cur_ext = ext_from_path(&dl.text); // subsequent lines in-window are colored with this file's extension
                file_header_line(&dl.text, width)
            } else {
                diff_line_to_line(dl, ranges[i], &cur_ext, theme)
            }
        })
        .collect()
}

/// Turn the whole diff (a sequence of DiffLines) into decorated lines — every row, highlighted.
/// Used by the commit-detail view (an unwindowed, bounded diff) and by tests; the GitDiff preview
/// itself uses `diff_lines_range` so it only highlights what's on screen. `default_ext` = syntax
/// detection for a single file, `theme` = color scheme, `width` = inner width to make framed
/// headers full width. File boundary headers are drawn framed, and **subsequent lines are
/// syntax-highlighted with that file's extension** (per-file coloring in multi-file diffs). From
/// pairs of changed lines, compute the "range of changed characters" and brighten the background
/// only for those characters.
pub fn diff_lines(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
) -> Vec<Line<'static>> {
    diff_lines_range(diff, default_ext, theme, width, 0, diff.len())
}

/// Maximum columns the **unified** body can be horizontally scrolled at display width `width`,
/// without highlighting any row (mirrors `side_by_side_max_hscroll`, which is likewise
/// highlight-free). A content row's rendered width is exactly the fixed gutter prefix (1
/// change-bar column + 2×`GUTTER_W`-wide line numbers + 2 separating spaces) plus the raw text's
/// display width — highlighting (`highlight_line_by_ext`) only re-styles substrings of `dl.text`,
/// it never adds, removes, or reflows characters, so this needs no highlighting to match what
/// `diff_line_to_line` actually renders (checked directly against `diff_lines`' built widths in
/// `unified_max_hscroll_matches_the_widest_built_line`). A file-header row can be wider than
/// `width` when the path itself doesn't fit (`file_header_line` still emits the whole label) — its
/// width is computed with the same math `file_header_line` uses.
pub fn unified_max_hscroll(diff: &[DiffLine], width: usize) -> usize {
    const PREFIX_W: usize = 1 + GUTTER_W + 1 + GUTTER_W + 1; // bar + old + sp + new + sp
    let max_content = diff
        .iter()
        .map(|dl| {
            if is_file_header(dl) {
                // Mirrors file_header_line's width math: "┌─" (2) + label + "┐" (1), padded to
                // `width` with "─" fill when the label is narrower than that.
                let label_w = UnicodeWidthStr::width(format!(" {} ", dl.text).as_str());
                (2 + label_w + 1).max(width)
            } else {
                PREFIX_W + UnicodeWidthStr::width(dl.text.as_str())
            }
        })
        .max()
        .unwrap_or(0);
    max_content.saturating_sub(width)
}

// ---- side-by-side -------------------------------------------------

/// One side cell of a side-by-side diff. `no` = line number, `bg`/`bar` = kind color (None = Context), `hl` = changed-char range,
/// `ext` = extension of the file this line belongs to (for syntax highlighting).
#[derive(Clone)]
struct Half {
    no: Option<u32>,
    text: String,
    bg: Option<Color>,
    bar: Option<Color>,
    /// Range of changed characters within the line (in chars). If Some, brighten only that part's background when rendering.
    hl: Option<(usize, usize)>,
    ext: String,
}

/// One row of a side-by-side diff. Header = file boundary (full width), Pair = left (old) / right (new).
enum SideRow {
    Header(String),
    Pair(Option<Half>, Option<Half>),
}

/// Total row count for the side-by-side layout at `default_ext`. Needed to clamp vertical scroll
/// **before** slicing which rows to render — unlike the unified layout, a side-by-side row doesn't
/// correspond 1:1 to a `DiffLine` (an unequal Removed/Added block folds several `DiffLine`s into
/// fewer rows via `flush_block`, since a row is `max(removed_in_block, added_in_block)`, not the
/// sum — see `diff_lines_side_by_side_range_matches_full_render_across_scroll_positions`, which
/// checks `total < diff.len()` for a fixture built with an unequal block), so the total can't be
/// read off `diff.len()`. Delegates to `build_side_rows`, which does the block-folding +
/// `intra_ranges` but never calls `highlight_line_by_ext` (that only happens in
/// `render_side_row`/`render_half`) — measured ~1.8ms for 6,500 rows (mostly the `Half` string
/// clones), negligible next to the highlighting cost it lets the renderer skip.
pub fn side_by_side_row_count(diff: &[DiffLine], default_ext: &str) -> usize {
    build_side_rows(diff, default_ext).len()
}

/// Render only the visible window `[start, start+count)` of the side-by-side row layout (see
/// `side_by_side_row_count` for why a row index isn't a `DiffLine` index). Highlighting
/// (`highlight_line_by_ext`, via `render_side_row` → `render_half`) only runs for the rows in the
/// window — the same windowing `diff_lines_range` does for the unified layout, and for the same
/// reason (measured ~60µs/row × up to 2 halves/row).
///
/// `build_side_rows` (block-folding + `intra_ranges`, both highlight-free) still runs over the
/// **whole** diff on every call, for the same correctness reason `diff_lines_range` keeps
/// `intra_ranges` whole: a change block's Removed/Added pairing — and, here, its **row-folding**
/// too — can start above `start`, so a partial view of the diff would produce a different (wrong)
/// layout, not just a differently-highlighted one.
pub fn diff_lines_side_by_side_range(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
    hscroll: usize,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    let sep_w = 1usize;
    let left_w = width.saturating_sub(sep_w) / 2;
    let right_w = width.saturating_sub(left_w + sep_w);
    build_side_rows(diff, default_ext)
        .into_iter()
        .skip(start)
        .take(count)
        .map(|row| render_side_row(row, theme, left_w, right_w, hscroll))
        .collect()
}

/// Turn the diff into **side-by-side (Zed-style)** lines — every row, highlighted. Used by the
/// commit-detail view and tests; the GitDiff preview itself uses `diff_lines_side_by_side_range`
/// so it only highlights what's on screen. `width` = column count of the body area. Left = old
/// (deletions in red), right = new (additions in green). Context appears identically on both sides
/// (line numbers per side). Change blocks pair deletions/additions top-down and fill the remainder
/// with empty cells. Each column has a fixed width, with a `│` separator in the middle.
pub fn diff_lines_side_by_side(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
    hscroll: usize,
) -> Vec<Line<'static>> {
    diff_lines_side_by_side_range(diff, default_ext, theme, width, hscroll, 0, diff.len())
}

/// Maximum columns the body can be horizontally scrolled in side-by-side view (= longest body display width − one column's body budget). For clamping on the render side.
pub fn side_by_side_max_hscroll(diff: &[DiffLine], width: usize) -> usize {
    let left_w = width.saturating_sub(1) / 2;
    let budget = left_w.saturating_sub(GUTTER_W + 2); // estimate using the narrower (left column) body budget
    let max_content = diff
        .iter()
        .filter(|dl| !is_file_header(dl))
        .map(|dl| UnicodeWidthStr::width(dl.text.as_str()))
        .max()
        .unwrap_or(0);
    max_content.saturating_sub(budget)
}

/// Fold a unified sequence of DiffLines into side-by-side rows. Changed lines carry their "range of changed characters" via intra_ranges, and
/// after a file boundary header each Half gets that file's extension (for coloring).
fn build_side_rows(diff: &[DiffLine], default_ext: &str) -> Vec<SideRow> {
    let ranges = intra_ranges(diff);
    let mut cur_ext = default_ext.to_string();
    let mut rows = Vec::new();
    let mut rem: Vec<Half> = Vec::new();
    let mut add: Vec<Half> = Vec::new();
    for (i, dl) in diff.iter().enumerate() {
        match dl.kind {
            DiffLineKind::Removed => {
                // If another deletion arrives after an addition, it's a separate block → finalize the previous one first.
                if !add.is_empty() {
                    flush_block(&mut rows, &mut rem, &mut add);
                }
                rem.push(Half {
                    no: dl.old_no,
                    text: dl.text.clone(),
                    bg: Some(BG_REMOVED),
                    bar: Some(BAR_REMOVED),
                    hl: ranges[i],
                    ext: cur_ext.clone(),
                });
            }
            DiffLineKind::Added => add.push(Half {
                no: dl.new_no,
                text: dl.text.clone(),
                bg: Some(BG_ADDED),
                bar: Some(BAR_ADDED),
                hl: ranges[i],
                ext: cur_ext.clone(),
            }),
            DiffLineKind::Context => {
                flush_block(&mut rows, &mut rem, &mut add);
                if is_file_header(dl) {
                    cur_ext = ext_from_path(&dl.text); // subsequently colored with this file's extension
                    rows.push(SideRow::Header(dl.text.clone()));
                } else {
                    let mk = |no| Half {
                        no,
                        text: dl.text.clone(),
                        bg: None,
                        bar: None,
                        hl: None,
                        ext: cur_ext.clone(),
                    };
                    rows.push(SideRow::Pair(Some(mk(dl.old_no)), Some(mk(dl.new_no))));
                }
            }
        }
    }
    flush_block(&mut rows, &mut rem, &mut add);
    rows
}

/// Pair up the accumulated deletions/additions top-down into rows. The remainder becomes empty cells (None).
fn flush_block(rows: &mut Vec<SideRow>, rem: &mut Vec<Half>, add: &mut Vec<Half>) {
    let n = rem.len().max(add.len());
    for i in 0..n {
        rows.push(SideRow::Pair(rem.get(i).cloned(), add.get(i).cloned()));
    }
    rem.clear();
    add.clear();
}

fn render_side_row(
    row: SideRow,
    theme: &str,
    left_w: usize,
    right_w: usize,
    hscroll: usize,
) -> Line<'static> {
    match row {
        // A file boundary is a full-width (left + separator + right) framed header (does not scroll horizontally).
        SideRow::Header(text) => file_header_line(&text, left_w + 1 + right_w),
        SideRow::Pair(left, right) => {
            let mut spans = render_half(left, theme, left_w, hscroll);
            spans.push(Span::styled("│", Style::new().fg(Color::DarkGray)));
            spans.extend(render_half(right, theme, right_w, hscroll));
            Line::from(spans)
        }
    }
}

/// Fit one side's column into the fixed width `col_w` as "line number + change bar + colored body" (the kind color background spans the full width).
/// Syntax highlighting uses the Half's `ext`. `hscroll` = columns to shift only the body horizontally (gutter/separator stay fixed).
/// An empty cell (None) is col_w of spaces.
fn render_half(
    half: Option<Half>,
    theme: &str,
    col_w: usize,
    hscroll: usize,
) -> Vec<Span<'static>> {
    let Some(h) = half else {
        return vec![Span::raw(" ".repeat(col_w))];
    };
    let bg = h.bg;
    let no =
        h.no.map(|n| format!("{n:>GUTTER_W$}"))
            .unwrap_or_else(|| " ".repeat(GUTTER_W));
    let dim = {
        let s = Style::new().fg(Color::DarkGray);
        if let Some(b) = bg {
            s.bg(b)
        } else {
            s
        }
    };
    // Place the change bar (1 column) **at the front = left of the line numbers** (fixed up to here = does not scroll horizontally).
    let bar_style = {
        let mut s = Style::new();
        if let Some(c) = h.bar {
            s = s.fg(c);
        }
        if let Some(b) = bg {
            s = s.bg(b);
        }
        s
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        if h.bar.is_some() { "▌" } else { " " }.to_string(),
        bar_style,
    ));
    spans.push(Span::styled(no, dim));
    spans.push(Span::styled(" ", dim));
    // Body: syntax-color → overlay the in-line emphasis (background) across **the whole text** →
    //       carve out the hscroll window [hscroll, hscroll+budget) → pad the remainder with the
    //       background color. This lets only the body move horizontally while the gutter/separator stay fixed.
    let budget = col_w.saturating_sub(GUTTER_W + 2); // gutter + bar + space
    let content = if h.text.is_empty() {
        Vec::new()
    } else {
        record_highlight_call();
        crate::preview::code::highlight_line_by_ext(&h.text, &h.ext, theme)
    };
    let content = if let Some(base) = bg {
        let strong = if base == BG_ADDED {
            BG_ADDED_STRONG
        } else {
            BG_REMOVED_STRONG
        };
        overlay_intra_bg(content, base, strong, h.hl)
    } else {
        content // Context: no background
    };
    let (clipped, used) = clip_spans_window(content, hscroll, budget);
    spans.extend(clipped);
    let pad = budget.saturating_sub(used);
    if pad > 0 {
        let pad_style = bg.map(|b| Style::new().bg(b)).unwrap_or_default();
        spans.push(Span::styled(" ".repeat(pad), pad_style));
    }
    spans
}

/// Clip a span sequence by display width into the **window [skip, skip+take)** (skip columns, then take columns).
/// With `skip`=0 this matches the old "take columns from the start". Full-width chars straddling the boundary are dropped (in code/ASCII
/// 1 column = 1 char so there is no effect). Returns (span sequence, display width taken).
fn clip_spans_window(
    spans: Vec<Span<'static>>,
    skip: usize,
    take: usize,
) -> (Vec<Span<'static>>, usize) {
    let end = skip.saturating_add(take);
    let mut out = Vec::new();
    let mut col = 0usize; // display column within the original content
    let mut used = 0usize; // display columns taken within the window
    for sp in spans {
        if col >= end {
            break;
        }
        let mut buf = String::new();
        for ch in sp.content.chars() {
            if col >= end {
                break;
            }
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            // Only take characters that fit **entirely** within the window [skip, end).
            if col >= skip && col + cw <= end {
                buf.push(ch);
                used += cw;
            }
            col += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, sp.style));
        }
    }
    (out, used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffLine, DiffLineKind};

    fn bg_colors(line: &Line<'static>) -> Vec<Option<Color>> {
        line.spans.iter().map(|s| s.style.bg).collect()
    }

    #[test]
    fn added_line_has_green_bg_and_bar() {
        let dl = DiffLine {
            kind: DiffLineKind::Added,
            old_no: None,
            new_no: Some(3),
            text: "let x = 1;".into(),
        };
        let line = diff_line_to_line(&dl, None, "rs", "TwoDark");
        // The Added background shows up on some span.
        assert!(
            bg_colors(&line).contains(&Some(BG_ADDED)),
            "Added 背景が無い"
        );
        // The change bar "▌" is present with green fg.
        assert!(
            line.spans
                .iter()
                .any(|s| s.content.as_ref() == "▌" && s.style.fg == Some(BAR_ADDED)),
            "緑の変更バーが無い"
        );
    }

    #[test]
    fn removed_line_has_red_bg() {
        let dl = DiffLine {
            kind: DiffLineKind::Removed,
            old_no: Some(2),
            new_no: None,
            text: "let y = 2;".into(),
        };
        let line = diff_line_to_line(&dl, None, "rs", "TwoDark");
        assert!(
            bg_colors(&line).contains(&Some(BG_REMOVED)),
            "Removed 背景が無い"
        );
    }

    #[test]
    fn side_by_side_splits_old_left_new_right() {
        use DiffLineKind::*;
        let diff = vec![
            DiffLine {
                kind: Context,
                old_no: Some(1),
                new_no: Some(1),
                text: "ctx".into(),
            },
            DiffLine {
                kind: Removed,
                old_no: Some(2),
                new_no: None,
                text: "old line".into(),
            },
            DiffLine {
                kind: Added,
                old_no: None,
                new_no: Some(2),
                text: "new line".into(),
            },
        ];
        let lines = diff_lines_side_by_side(&diff, "rs", "TwoDark", 60, 0);
        for l in &lines {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            assert!(s.contains('│'), "区切り │ が無い: {s}");
            // Each column has a fixed width, so the whole line fits within width=60.
            assert!(UnicodeWidthStr::width(s.as_str()) <= 60, "幅超過: {s}");
        }
        // Changed line: old line (Removed) is left of │, new line (Added) is right of │ (= side by side).
        let change: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .find(|s| s.contains("old line"))
            .expect("変更行が無い");
        let bar = change.find('│').unwrap();
        assert!(change.find("old line").unwrap() < bar, "old は左: {change}");
        assert!(change.find("new line").unwrap() > bar, "new は右: {change}");
        // A red background (BG_REMOVED) sits on the left, and a green background (BG_ADDED) on the right (fine if in-line emphasis splits the span).
        let row = lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("old line")
            })
            .unwrap();
        assert!(
            row.spans
                .iter()
                .any(|s| s.style.bg == Some(BG_REMOVED) || s.style.bg == Some(BG_REMOVED_STRONG)),
            "左に赤背景が無い"
        );
        assert!(
            row.spans
                .iter()
                .any(|s| s.style.bg == Some(BG_ADDED) || s.style.bg == Some(BG_ADDED_STRONG)),
            "右に緑背景が無い"
        );
    }

    #[test]
    fn side_by_side_hscroll_moves_content_not_gutter() {
        use DiffLineKind::*;
        let long = format!("START{}END", "x".repeat(40)); // 48 columns
        let diff = vec![DiffLine {
            kind: Context,
            old_no: Some(7),
            new_no: Some(7),
            text: long,
        }];
        let row = |hscroll: usize| -> String {
            diff_lines_side_by_side(&diff, "txt", "TwoDark", 40, hscroll)[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        // hscroll=0: START visible, END invisible. Line number 7 and separator │ appear.
        let s0 = row(0);
        assert!(s0.contains("START") && !s0.contains("END"), "0: {s0}");
        assert!(
            s0.contains('7') && s0.contains('│'),
            "行番号/区切りが出る: {s0}"
        );
        // Scrolled horizontally to the max: END visible, START invisible. **Line number 7 and separator │ stay fixed**.
        let max = side_by_side_max_hscroll(&diff, 40);
        assert!(max > 0, "横スクロール可能幅がある");
        let se = row(max);
        assert!(se.contains("END") && !se.contains("START"), "max: {se}");
        assert!(
            se.contains('7') && se.contains('│'),
            "横移動してもガター/区切りは固定: {se}"
        );
    }

    #[test]
    fn intra_line_highlights_only_changed_chars() {
        use DiffLineKind::*;
        let diff = vec![
            DiffLine {
                kind: Removed,
                old_no: Some(2),
                new_no: None,
                text: "    let x = 1;".into(),
            },
            DiffLine {
                kind: Added,
                old_no: None,
                new_no: Some(2),
                text: "    let x = 2;".into(),
            },
        ];
        // unified: on the deletion line, '1' has the strong background while the rest of the body is base. On the addition line, '2' is strong.
        let lines = diff_lines(&diff, "rs", "TwoDark", 80);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content.as_ref() == "1" && s.style.bg == Some(BG_REMOVED_STRONG)),
            "削除行: 変更文字 '1' が明るい背景でない"
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.bg == Some(BG_REMOVED)),
            "削除行: 通常の赤背景が無い(全部 strong になっている)"
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|s| s.content.as_ref() == "2" && s.style.bg == Some(BG_ADDED_STRONG)),
            "追加行: 変更文字 '2' が明るい背景でない"
        );

        // side-by-side: the same row has left '1' (strong red) and right '2' (strong green).
        let sbs = diff_lines_side_by_side(&diff, "rs", "TwoDark", 60, 0);
        let has_red_strong = sbs.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content.as_ref() == "1" && s.style.bg == Some(BG_REMOVED_STRONG))
        });
        let has_green_strong = sbs.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content.as_ref() == "2" && s.style.bg == Some(BG_ADDED_STRONG))
        });
        assert!(
            has_red_strong && has_green_strong,
            "横並びでも変更文字が明るい背景"
        );
    }

    #[test]
    fn file_header_is_framed_and_per_file_syntax_applies() {
        use DiffLineKind::*;
        let diff = vec![
            // A file boundary header (line numbers None, a plain path).
            DiffLine {
                kind: Context,
                old_no: None,
                new_no: None,
                text: "src/main.rs".into(),
            },
            DiffLine {
                kind: Context,
                old_no: Some(1),
                new_no: Some(1),
                text: "fn main() {}".into(),
            },
        ];
        let lines = diff_lines(&diff, "", "TwoDark", 60);
        // The first is a framed header: ╭ … path … ╮ in blue, with no `──` decoration.
        let hdr: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            hdr.starts_with('┌') && hdr.ends_with('┐'),
            "枠ヘッダでない: {hdr}"
        );
        assert!(hdr.contains("src/main.rs"), "パスが無い: {hdr}");
        assert!(
            lines[0].spans.iter().any(|s| s.style.fg == Some(HEADER_FG)),
            "枠が青(HEADER_FG)でない"
        );
        assert!(!hdr.contains("── "), "旧 `── ` 装飾が残っている");
        // After the header, colored with that file's (.rs) syntax → the body has multiple Rgb foreground colors.
        let colors: std::collections::HashSet<(u8, u8, u8)> = lines[1]
            .spans
            .iter()
            .filter_map(|s| match s.style.fg {
                Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
                _ => None,
            })
            .collect();
        assert!(
            colors.len() >= 2,
            "rs として構文着色されていない (色数 {})",
            colors.len()
        );
    }

    #[test]
    fn context_line_has_no_row_bg() {
        let dl = DiffLine {
            kind: DiffLineKind::Context,
            old_no: Some(1),
            new_no: Some(1),
            text: "fn main() {".into(),
        };
        let line = diff_line_to_line(&dl, None, "rs", "TwoDark");
        // Context lays no diff background (every span's bg is None).
        assert!(
            bg_colors(&line).iter().all(|b| b.is_none()),
            "Context に背景が乗っている"
        );
        // No literal +/- is shown.
        let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!joined.trim_start().starts_with('+'));
        assert!(!joined.contains("─")); // not a header rule line either
    }

    #[test]
    fn side_by_side_empty_cell_and_hscroll_paths() {
        use DiffLineKind::*;
        // 2 deletion lines, 1 addition line → on the 2nd line the right side (add) is an empty cell (None = render_half's empty-cell path).
        let diff = vec![
            DiffLine {
                kind: Removed,
                old_no: Some(2),
                new_no: None,
                text: "removed first line".into(),
            },
            DiffLine {
                kind: Removed,
                old_no: Some(3),
                new_no: None,
                text: "removed second line".into(),
            },
            DiffLine {
                kind: Added,
                old_no: None,
                new_no: Some(2),
                text: "added only line".into(),
            },
        ];
        // hscroll=0: the empty-cell path. Each row has the separator │ and fits within width 60.
        let lines = diff_lines_side_by_side(&diff, "rs", "TwoDark", 60, 0);
        assert!(!lines.is_empty());
        for l in &lines {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            assert!(s.contains('│'), "区切りが無い: {s}");
            assert!(UnicodeWidthStr::width(s.as_str()) <= 60, "幅超過: {s}");
        }
        // The "removed second line" row: the right side (new) is an empty cell → only blanks after │.
        let row: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .find(|s| s.contains("removed second"))
            .expect("削除2行目が無い");
        let bar = row.find('│').unwrap();
        let right = &row[bar + '│'.len_utf8()..];
        assert!(right.trim().is_empty(), "右側は空セル(空白): {right:?}");

        // hscroll>0: also exercise render_half's path where only the body shifts horizontally (width is preserved).
        let shifted = diff_lines_side_by_side(&diff, "rs", "TwoDark", 60, 6);
        assert!(!shifted.is_empty());
        for l in &shifted {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            assert!(
                UnicodeWidthStr::width(s.as_str()) <= 60,
                "横スクロール後も幅 60 以内: {s}"
            );
        }
    }

    // ---- windowed (visible-range) rendering: the perf fix ---------------------------------
    //
    // `diff_lines`/`diff_lines_side_by_side` highlight (syntect, via `highlight_line_by_ext`)
    // *every* row regardless of what's actually on screen — measured ~60µs/row, ~400–760ms for a
    // real 6,500-row diff, opened every keypress (the frame budget is 60ms). `diff_lines_range` /
    // `diff_lines_side_by_side_range` below must highlight only the rows they actually return.

    /// A synthetic diff spanning `files` file headers, each with `lines_per_file` iterations
    /// producing a mix of context / paired Removed+Added (word-level changes) / lone Added rows —
    /// enough variety in one fixture to exercise file-header extension threading, intra-line
    /// highlight pairing, and unequal remove/add block folding (side-by-side row count ≠
    /// `diff.len()`) all at once.
    fn synthetic_diff(files: usize, lines_per_file: usize) -> Vec<DiffLine> {
        use DiffLineKind::*;
        let exts = ["rs", "py"];
        let mut diff = Vec::new();
        for f in 0..files {
            let ext = exts[f % exts.len()];
            diff.push(DiffLine {
                kind: Context,
                old_no: None,
                new_no: None,
                text: format!("src/file{f}.{ext}"),
            });
            let mut old_no = 1u32;
            let mut new_no = 1u32;
            for i in 0..lines_per_file {
                match i % 5 {
                    0 | 1 => {
                        diff.push(DiffLine {
                            kind: Context,
                            old_no: Some(old_no),
                            new_no: Some(new_no),
                            text: format!("    ctx line {i} in file {f}"),
                        });
                        old_no += 1;
                        new_no += 1;
                    }
                    2 | 3 => {
                        diff.push(DiffLine {
                            kind: Removed,
                            old_no: Some(old_no),
                            new_no: None,
                            text: format!("    let value = {i}; // old file {f}"),
                        });
                        old_no += 1;
                        diff.push(DiffLine {
                            kind: Added,
                            old_no: None,
                            new_no: Some(new_no),
                            text: format!("    let value = {}; // new file {f}", i * 2),
                        });
                        new_no += 1;
                    }
                    _ => {
                        diff.push(DiffLine {
                            kind: Added,
                            old_no: None,
                            new_no: Some(new_no),
                            text: format!("    // appended note {i} in file {f}"),
                        });
                        new_no += 1;
                    }
                }
            }
        }
        diff
    }

    #[test]
    fn windowed_range_only_highlights_visible_rows_unified() {
        let diff = synthetic_diff(2, 400); // 1,000+ rows, far larger than any real viewport
        reset_highlight_calls();
        let window = diff_lines_range(&diff, "txt", "TwoDark", 100, 500, 40);
        let calls = highlight_calls();
        assert_eq!(window.len(), 40, "要求した高さぶんだけ返る");
        assert!(
            calls > 0 && calls <= 80,
            "可視範囲(高さ40)を大きく超えてハイライトしている(全 {} 行中 {calls} 回): O(diff全体)に戻っていないか",
            diff.len()
        );
    }

    #[test]
    fn windowed_range_only_highlights_visible_rows_side_by_side() {
        let diff = synthetic_diff(2, 400);
        reset_highlight_calls();
        let window = diff_lines_side_by_side_range(&diff, "txt", "TwoDark", 100, 0, 300, 40);
        let calls = highlight_calls();
        assert_eq!(window.len(), 40, "要求した高さぶんだけ返る");
        // Each side-by-side row can highlight up to two halves (old + new), so allow 2x headroom.
        assert!(
            calls > 0 && calls <= 160,
            "可視範囲(高さ40×2列)を大きく超えてハイライトしている({calls} 回): O(diff全体)に戻っていないか"
        );
    }

    /// Render `lines` (already the desired vertical slice; `Paragraph` gets `scroll=(0, hscroll)`)
    /// into a `TestBackend` cell buffer for byte/style-exact comparison against a reference render.
    fn render_to_buffer(
        lines: Vec<Line<'static>>,
        width: u16,
        height: u16,
        hscroll: u16,
    ) -> ratatui::buffer::Buffer {
        use ratatui::layout::Rect;
        use ratatui::text::Text;
        use ratatui::widgets::Paragraph;
        use ratatui::Terminal;
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| {
            let para = Paragraph::new(Text::from(lines)).scroll((0, hscroll));
            f.render_widget(para, area);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    /// Same as `render_to_buffer`, but scrolls a **full, unsliced** line list by `scroll` (the
    /// "old implementation" shape: build everything, let `Paragraph` clip it).
    fn render_full_scrolled(
        lines: Vec<Line<'static>>,
        width: u16,
        height: u16,
        scroll: u16,
        hscroll: u16,
    ) -> ratatui::buffer::Buffer {
        use ratatui::layout::Rect;
        use ratatui::text::Text;
        use ratatui::widgets::Paragraph;
        use ratatui::Terminal;
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| {
            let para = Paragraph::new(Text::from(lines)).scroll((scroll, hscroll));
            f.render_widget(para, area);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn assert_buffers_match(
        actual: &ratatui::buffer::Buffer,
        reference: &ratatui::buffer::Buffer,
        width: u16,
        height: u16,
        ctx: &str,
    ) {
        for y in 0..height {
            for x in 0..width {
                let a = &actual[(x, y)];
                let r = &reference[(x, y)];
                assert_eq!(
                    (a.symbol(), a.fg, a.bg, a.modifier),
                    (r.symbol(), r.fg, r.bg, r.modifier),
                    "{ctx}: cell({x},{y}) がフル描画と不一致"
                );
            }
        }
    }

    #[test]
    fn diff_lines_range_matches_full_render_across_scroll_positions_and_file_boundary() {
        let diff = synthetic_diff(2, 400);
        let total = diff.len();
        let (iw, ih) = (72u16, 18u16);
        let full = diff_lines(&diff, "txt", "TwoDark", iw as usize);
        assert_eq!(full.len(), total, "diff_lines は1行=1DiffLineのまま");

        let file2_header = diff
            .iter()
            .position(|dl| is_file_header(dl) && dl.text.ends_with(".py"))
            .expect("2つ目のファイルヘッダがある");
        let max_v = total.saturating_sub(ih as usize);
        // 0=先頭 / 37=中間 / file2_header の直前・直後(ヘッダ跨ぎ)/ 末尾ページ。
        let scrolls = [
            0usize,
            37,
            file2_header.saturating_sub(2),
            file2_header,
            file2_header + 3,
            max_v,
        ];

        for scroll in scrolls {
            let window =
                diff_lines_range(&diff, "txt", "TwoDark", iw as usize, scroll, ih as usize);
            let actual = render_to_buffer(window, iw, ih, 0);
            let reference = render_full_scrolled(full.clone(), iw, ih, scroll as u16, 0);
            assert_buffers_match(&actual, &reference, iw, ih, &format!("scroll={scroll}"));
        }
    }

    #[test]
    fn diff_lines_range_preserves_intra_pairing_when_window_splits_a_change_block() {
        // A single block: 30 Removed then 30 Added (each a word-level change on the same
        // "value_K = …" line). If the windowed path computed `intra_ranges` only over the visible
        // slice instead of the whole diff, a window that starts mid-block would lose the removed
        // lines *before* it that determine the pairing offset, mispairing Removed/Added lines and
        // producing a different (wrong) "changed characters" range than the full render.
        use DiffLineKind::*;
        let mut diff = Vec::new();
        for k in 0..30u32 {
            diff.push(DiffLine {
                kind: Removed,
                old_no: Some(k + 1),
                new_no: None,
                text: format!("value_{k} = OLD_{k};"),
            });
        }
        for k in 0..30u32 {
            diff.push(DiffLine {
                kind: Added,
                old_no: None,
                new_no: Some(k + 1),
                text: format!("value_{k} = NEW_{k}_CHANGED;"),
            });
        }
        let (iw, ih) = (60u16, 20u16);
        let full = diff_lines(&diff, "txt", "TwoDark", iw as usize);

        // The window starts 15 rows in (mid-Removed-run) and ends 5 rows into the Added run.
        let window = diff_lines_range(&diff, "txt", "TwoDark", iw as usize, 15, 20);
        let actual = render_to_buffer(window, iw, ih, 0);
        let reference = render_full_scrolled(full, iw, ih, 15, 0);
        assert_buffers_match(&actual, &reference, iw, ih, "block-straddling window");
    }

    #[test]
    fn diff_lines_side_by_side_range_matches_full_render_across_scroll_positions() {
        let diff = synthetic_diff(2, 400);
        let (iw, ih) = (72u16, 18u16);
        let ext = "txt";
        let full = diff_lines_side_by_side(&diff, ext, "TwoDark", iw as usize, 0);
        let total = side_by_side_row_count(&diff, ext);
        assert_eq!(
            full.len(),
            total,
            "row_count はフル描画の行数と一致するはず"
        );
        assert!(
            total < diff.len(),
            "不揃いな remove/add ブロックの折り畳みで行数は diff.len() 未満になるはず(区別できているか): total={total} diff.len()={}",
            diff.len()
        );
        let max_v = total.saturating_sub(ih as usize);
        for scroll in [0usize, 20, max_v / 2, max_v] {
            let window = diff_lines_side_by_side_range(
                &diff,
                ext,
                "TwoDark",
                iw as usize,
                0,
                scroll,
                ih as usize,
            );
            let actual = render_to_buffer(window, iw, ih, 0);
            let reference = render_full_scrolled(full.clone(), iw, ih, scroll as u16, 0);
            assert_buffers_match(&actual, &reference, iw, ih, &format!("sbs scroll={scroll}"));
        }
    }

    #[test]
    fn diff_lines_side_by_side_range_matches_full_render_with_hscroll() {
        // The body-only horizontal scroll (independent from the vertical window) must still line
        // up: the windowed render at hscroll=H must equal the full render's row at the same
        // logical row, also drawn at hscroll=H (side-by-side's hscroll is applied per-row inside
        // `render_half`, not via Paragraph, so this exercises a different code path than the
        // vertical-only test above).
        let diff = synthetic_diff(1, 200);
        let (iw, ih) = (50u16, 12u16);
        let ext = "txt";
        for hscroll in [0usize, 6] {
            let full = diff_lines_side_by_side(&diff, ext, "TwoDark", iw as usize, hscroll);
            let total = side_by_side_row_count(&diff, ext);
            let max_v = total.saturating_sub(ih as usize);
            let window = diff_lines_side_by_side_range(
                &diff,
                ext,
                "TwoDark",
                iw as usize,
                hscroll,
                max_v / 2,
                ih as usize,
            );
            let actual = render_to_buffer(window, iw, ih, 0);
            let reference = render_full_scrolled(full, iw, ih, (max_v / 2) as u16, 0);
            assert_buffers_match(&actual, &reference, iw, ih, &format!("hscroll={hscroll}"));
        }
    }

    #[test]
    fn unified_max_hscroll_matches_the_widest_built_line() {
        // Cross-check the highlight-free width estimate against the actual widths `diff_lines`
        // (built with highlighting) produces — must match exactly, since highlighting only
        // re-styles substrings and never changes displayed text or width.
        let diff = synthetic_diff(2, 60);
        for width in [40usize, 80, 120] {
            let lines = diff_lines(&diff, "txt", "TwoDark", width);
            let expected = lines
                .iter()
                .map(|l| l.width())
                .max()
                .unwrap_or(0)
                .saturating_sub(width);
            let actual = unified_max_hscroll(&diff, width);
            assert_eq!(actual, expected, "width={width}");
        }
    }

    #[test]
    fn unified_total_rows_is_diff_len_with_headers() {
        // Documents the invariant `diff_lines_range` relies on for O(1) total-row-count: one
        // `DiffLine` (headers included) always maps to exactly one unified output row.
        let diff = synthetic_diff(3, 25);
        assert_eq!(diff_lines(&diff, "txt", "TwoDark", 80).len(), diff.len());
    }
}
