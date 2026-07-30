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
        crate::preview::code::highlight_line_by_ext(&dl.text, ext, theme)
    };
    match (row_bg, strong_bg) {
        (Some(base), Some(strong)) => spans.extend(overlay_intra_bg(content, base, strong, intra)),
        _ => spans.extend(content), // Context: no background
    }

    Line::from(spans)
}

/// Turn the whole diff (a sequence of DiffLines) into decorated lines. `default_ext` = syntax detection for a single file,
/// `theme` = color scheme, `width` = inner width to make framed headers full width. File boundary headers are drawn framed, and
/// **subsequent lines are syntax-highlighted with that file's extension** (per-file coloring in multi-file diffs).
/// From pairs of changed lines, compute the "range of changed characters" and brighten the background only for those characters.
pub fn diff_lines(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let ranges = intra_ranges(diff);
    let mut cur_ext = default_ext.to_string();
    diff.iter()
        .enumerate()
        .map(|(i, dl)| {
            if is_file_header(dl) {
                cur_ext = ext_from_path(&dl.text); // subsequent lines are colored with this file's extension
                file_header_line(&dl.text, width)
            } else {
                diff_line_to_line(dl, ranges[i], &cur_ext, theme)
            }
        })
        .collect()
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

/// Turn the diff into **side-by-side (Zed-style)** lines. `width` = column count of the body area.
/// Left = old (deletions in red), right = new (additions in green). Context appears identically on both sides (line numbers per side). Change blocks
/// pair deletions/additions top-down and fill the remainder with empty cells. Each column has a fixed width, with a `│` separator in the middle.
pub fn diff_lines_side_by_side(
    diff: &[DiffLine],
    default_ext: &str,
    theme: &str,
    width: usize,
    hscroll: usize,
) -> Vec<Line<'static>> {
    let sep_w = 1usize;
    let left_w = width.saturating_sub(sep_w) / 2;
    let right_w = width.saturating_sub(left_w + sep_w);
    build_side_rows(diff, default_ext)
        .into_iter()
        .map(|row| render_side_row(row, theme, left_w, right_w, hscroll))
        .collect()
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
}
