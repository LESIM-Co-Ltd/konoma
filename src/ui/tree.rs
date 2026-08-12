//! Tree rendering (full-screen).
//! M0: render the flat-expanded entries with a leading icon and reverse the selected row.
//! Icons are uncolored = they inherit the terminal's default foreground (mostly monochrome). In the future, color is
//! applied only to entries with a git status (FR-7). Large-scale support (lazy loading, scroll follow) comes later.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::fileops;
use crate::i18n::{tr, Msg};
use crate::ui::icons;
use crate::ui::status::{hint, page_hint};

/// Truncate to fit display width `max` (append `…` at the end when over). Returns (string, display width).
fn truncate_width(s: &str, max: usize) -> (String, usize) {
    let w = s.width();
    if w <= max {
        return (s.to_string(), w);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > max - 1 {
            break; // reserve room for the trailing `…` (width 1)
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    (out, used + 1)
}

/// Within the filter result's display name `name`, build a span sequence that highlights the parts matching
/// the query `query` (case-insensitive). Non-matching parts use `base`; matching parts are yellow + bold.
///
/// Lowercasing can change byte length and char count (e.g. `İ` → `i` + a combining dot = 2 chars, `ẞ` → `ß`).
/// So instead of slicing name by byte positions on the lowercased string, we map match positions directly
/// to name's **char boundaries**. This makes highlighting work even for non-ASCII names and avoids panics from non-char-boundary slices.
fn highlight_match(name: &str, query: &str, base: Style) -> Vec<Span<'static>> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return vec![Span::styled(name.to_string(), base)];
    }
    // Lowercase each character of name while keeping the byte range [start,end) it occupies in name.
    // to_lowercase can expand one character into several, so each lowercased unit is tied to the same name byte range.
    let mut units: Vec<(char, usize, usize)> = Vec::new();
    for (b, ch) in name.char_indices() {
        let end = b + ch.len_utf8();
        for lc in ch.to_lowercase() {
            units.push((lc, b, end));
        }
    }
    // Collect the name byte ranges that matched the query, ascending and non-overlapping (the range edges are always name's char boundaries).
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k + q.len() <= units.len() {
        if units[k..k + q.len()]
            .iter()
            .map(|u| u.0)
            .eq(q.iter().copied())
        {
            let start = units[k].1;
            let end = units[k + q.len() - 1].2;
            match ranges.last_mut() {
                // Merge when it touches/overlaps the previous range (avoid double-highlighting within the same character).
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => ranges.push((start, end)),
            }
            k += q.len();
        } else {
            k += 1;
        }
    }
    if ranges.is_empty() {
        return vec![Span::styled(name.to_string(), base)];
    }
    let hl = base.fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut i = 0;
    for (start, end) in ranges {
        if start > i {
            spans.push(Span::styled(name[i..start].to_string(), base));
        }
        spans.push(Span::styled(name[start..end].to_string(), hl));
        i = end;
    }
    if i < name.len() {
        spans.push(Span::styled(name[i..].to_string(), base));
    }
    spans
}

/// Context appendix for the Tree view (the mode chip is prepended by `status`). Sort order + selection count.
pub fn context(app: &App) -> Vec<Span<'static>> {
    // Current sort order (e.g. "sort: mod ↑").
    let mut spans = vec![Span::from(format!("  {}", app.sort_label())).dim()];
    // While selecting/in Visual, show the count (e.g. "sel: 3").
    if app.show_selection_gutter() {
        spans.push(
            Span::from(format!(
                "  {}: {}",
                tr(app.lang, crate::i18n::Msg::SelLabel),
                app.marked_count()
            ))
            .bold(),
        );
    }
    // Show when something is queued on the clipboard (e.g. "[copy 3]"). Signals that a paste is available.
    if let Some(label) = app.clipboard_label() {
        spans.push(Span::from(format!("  [{label}]")).dim());
    }
    spans
}

/// The Tree view's `?` help section (tree operations + git status markers). **Edit the Tree help here**.
pub fn help_sections(app: &App) -> Vec<crate::ui::help::HelpSection> {
    use crate::ui::help::HelpSection;
    let lang = app.lang;
    let l = |m| tr(lang, m);
    vec![
        HelpSection::new(l(crate::i18n::Msg::TreeSection))
            .row("j / k / ↑ ↓", l(crate::i18n::Msg::TreeMoveUpDown))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("l", l(crate::i18n::Msg::EnterDirectory))
            .row("Enter", l(crate::i18n::Msg::ExpandInPlace))
            .row("Ctrl-t", l(crate::i18n::Msg::OpenInNewTabHelp))
            .row("h", l(crate::i18n::Msg::ToParent))
            .row("a", l(crate::i18n::Msg::TreeAnchorRoot))
            .row("A", l(crate::i18n::Msg::ResetRoot))
            .row("d", l(crate::i18n::Msg::TreeDiffFile))
            .row("/", l(crate::i18n::Msg::TreeFilter))
            .row(".", l(crate::i18n::Msg::ToggleHidden))
            .row("i", l(crate::i18n::Msg::TreeFileInfo))
            .row("e", l(crate::i18n::Msg::EditExternalEnv))
            .row("o", l(crate::i18n::Msg::TreeGitChangesHub))
            .row("C", l(crate::i18n::Msg::ChangedFilterHelp))
            .row("n / N", l(crate::i18n::Msg::JumpChangeHelp))
            .row("F", l(crate::i18n::Msg::FollowHelp))
            .row("r", l(crate::i18n::Msg::Refresh))
            .row("s", l(crate::i18n::Msg::SortHint))
            .row("m / '", l(crate::i18n::Msg::TreeBookmarkHint))
            .row(crate::ui::status::page_help(app), ""),
        // File management (the Space leader) is built from the actual keymap (config already reflected).
        // Delete is y = trash / ! = permanent delete; create uses a trailing / for a folder.
        crate::ui::help::leader_section(app, crate::keymap::LeaderId::File, "Space")
            .unwrap_or_else(|| HelpSection::new(l(crate::i18n::Msg::TreeFile))),
        HelpSection::new(l(crate::i18n::Msg::TreeSelection))
            .row("v", l(crate::i18n::Msg::VisualRangeHint))
            .row("V", l(crate::i18n::Msg::ToggleOne))
            .row(
                l(crate::i18n::Msg::TreeDragDrop),
                l(crate::i18n::Msg::TreeDropFiles),
            ),
        HelpSection::new(l(crate::i18n::Msg::TreeGitStatus))
            .row("M", l(crate::i18n::Msg::TreeModified))
            .row("A", l(crate::i18n::Msg::TreeAddedStaged))
            .row("U", l(crate::i18n::Msg::Untracked))
            .row("D", l(crate::i18n::Msg::TreeDeleted))
            .row("R / T / !", l(crate::i18n::Msg::TreeStatusRenamed)),
    ]
}

/// The Tree view's footer key hints (high → low priority). **Edit here to change the Tree footer**.
/// It reads `&App`, so in the future it can also switch by cursor position or selection state.
pub fn footer_hints(app: &App) -> Vec<String> {
    let lang = app.lang;
    // With multiple tabs, q = close the tab (+ Q = quit); with the last one, q = quit.
    let multi = app.tab_count() > 1;
    let mut v = vec![
        hint(lang, "jk", crate::i18n::Msg::GitMove),
        hint(lang, "l", crate::i18n::Msg::HintEnter),
        hint(lang, "h", crate::i18n::Msg::HintUp),
        hint(
            lang,
            "q",
            if multi {
                crate::i18n::Msg::HintCloseTab
            } else {
                crate::i18n::Msg::HintQuit
            },
        ),
    ];
    if multi {
        v.push(hint(lang, "Q", crate::i18n::Msg::HintQuit));
    }
    v.extend([
        hint(lang, "?", crate::i18n::Msg::HintHelp),
        hint(lang, "/", crate::i18n::Msg::HintFilter),
        hint(lang, "Space", crate::i18n::Msg::HintFileOps),
        hint(lang, "e", crate::i18n::Msg::HintEdit),
        hint(lang, "o", crate::i18n::Msg::HintGit),
        hint(lang, "d", crate::i18n::Msg::HintDiff),
        hint(lang, "C", crate::i18n::Msg::StChangedOnly),
        hint(lang, "F", crate::i18n::Msg::StFollow),
        hint(lang, "y", crate::i18n::Msg::CopyHint),
        hint(lang, "t", crate::i18n::Msg::HintTab),
        hint(lang, "C-t", crate::i18n::Msg::HintNewTab),
        hint(lang, "[/]", crate::i18n::Msg::HintTab),
        hint(lang, "p", crate::i18n::Msg::HintPath),
        hint(lang, ".", crate::i18n::Msg::HintHidden),
        hint(lang, "i", crate::i18n::Msg::HintInfo),
        hint(lang, "s", crate::i18n::Msg::HintSort),
        hint(lang, "m", crate::i18n::Msg::HintMark),
        hint(lang, "'", crate::i18n::Msg::HintBookmarks),
        hint(lang, "P", crate::i18n::Msg::HintPasteJump),
        hint(lang, "v", crate::i18n::Msg::HintVisual),
        hint(lang, "V", crate::i18n::Msg::HintPick),
        hint(lang, "a", crate::i18n::Msg::HintAnchor),
        page_hint(app),
    ]);
    v
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // Re-validation now happens once per frame in `ui::render` (before this dispatch), covering
    // every mode/view, not just the tree — see the comment there.
    let icons_on = app.cfg.ui.icons;
    // If there is even one change, show a status gutter (2 columns) at the start of every row for alignment.
    let show_gutter = app.git_has_changes();
    // While multi-selecting/in Visual, show a selection-marker column (2 columns) at the leftmost of each row (skipped when empty).
    let show_sel = app.show_selection_gutter();
    // While filtering/showing changes-only, the result list is flat, so show each row's path relative to root so its location is clear.
    let filtering = app.filter_query().is_some() || app.changed_filter();
    let query = app.filter_query().unwrap_or("").to_string();
    let root_for_rel = app.tab.root.clone();
    // Detail list columns (config [ui] details). Only enabled columns are adopted, laid out at fixed width on the right edge.
    let detail_cols: Vec<String> = app
        .cfg
        .ui
        .details
        .iter()
        .filter(|id| fileops::detail_column_width(id).is_some())
        .cloned()
        .collect();
    let show_details = !detail_cols.is_empty();
    let details_w: usize = detail_cols
        .iter()
        .map(|id| 2 + fileops::detail_column_width(id).unwrap()) // "  " separator + column width
        .sum();
    let inner_w = area.width.saturating_sub(2) as usize; // left/right border
    let name_region_w = inner_w.saturating_sub(details_w); // the name + padding region

    // Only format the visible range (C3). Even with thousands of entries, limit formatting/`quick_meta`'s
    // stat syscall to the rows that actually appear on screen (≈ viewport height), avoiding a full
    // re-format on every scroll. Decide the scroll amount up front so the selected row is always
    // visible, then turn only `[offset..end]` into Lines.
    let visible = area.height.saturating_sub(2) as usize; // excluding the top/bottom border
    app.tab.tree_viewport = visible as u16; // used as one page's worth for paging
    let offset = if visible > 0 && app.tab.selected >= visible {
        app.tab.selected - visible + 1
    } else {
        0
    };
    let end = offset.saturating_add(visible).min(app.tab.entries.len());
    // Pre-pass: fill only the visible rows into App's (path → cells) detail-cell cache first.
    // Prevents the per-row stat (the `items` column is read_dir) from running on every keypress; discarded on tree rebuild.
    if show_details {
        let vis: Vec<std::path::PathBuf> = app.tab.entries[offset..end]
            .iter()
            .map(|e| e.path.clone())
            .collect();
        app.ensure_detail_cells(&vis, &detail_cols);
    }
    let lines: Vec<Line> = app.tab.entries[offset..end]
        .iter()
        .enumerate()
        .map(|(vi, e)| {
            let i = offset + vi; // the original index within entries (used for selection/Visual checks)
            let indent = "  ".repeat(e.depth);
            let fname = || {
                e.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string()
            };
            // While filtering, the path relative to root; otherwise the file name (as an owned String, put into a Line).
            let name: String = if filtering {
                e.path
                    .strip_prefix(&root_for_rel)
                    .ok()
                    .and_then(|p| p.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(fname)
            } else {
                fname()
            };
            let chevron = if e.is_dir {
                if e.expanded {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            let status = app.git_status_of(&e.path);
            let prefix = if icons_on {
                let glyph = if e.is_dir {
                    icons::dir_icon(e.expanded)
                } else {
                    icons::file_icon(&e.path)
                };
                format!("{indent}{chevron}{glyph}  ")
            } else {
                format!("{indent}{chevron}")
            };

            // Row start (before the indent) = multi-select marker → git status gutter. Color = meaning.
            let mut spans: Vec<Span> = Vec::new();
            if show_sel {
                // Show a mark for confirmed selection ∪ the in-progress Visual range.
                if app.is_selected(&e.path) || app.is_in_visual_range(i) {
                    spans.push(Span::from("● ").bold());
                } else {
                    spans.push(Span::from("  "));
                }
            }
            if show_gutter {
                match status {
                    Some(st) => spans.push(Span::styled(
                        format!("{} ", st.marker()),
                        Style::new().fg(st.color()),
                    )),
                    None => spans.push(Span::from("  ")),
                }
            }
            let prefix_w = prefix.width();
            // gitignore-excluded (self or an ancestor is ignored) is dimmed slightly (DIM), Zed-style.
            // An entry with a change status (color = meaning) keeps its color as before (not treated as ignored).
            let ignored = status.is_none() && app.is_ignored(&e.path);
            let dim = Style::new().add_modifier(Modifier::DIM);
            spans.push(Span::styled(
                prefix,
                if ignored { dim } else { Style::default() },
            ));
            // Name color priority: git status color > ignored (dimmed) > default color (follows theme).
            let name_style = if let Some(st) = status {
                Style::new().fg(st.color())
            } else if ignored {
                dim
            } else {
                Style::default()
            };
            if show_details {
                // Pack the name into the region width, pad the rest with spaces, then lay the fixed-width columns out on the right edge (vertically aligned).
                let left_w = (show_sel as usize) * 2 + (show_gutter as usize) * 2 + prefix_w;
                let budget = name_region_w.saturating_sub(left_w);
                let (tname, nw) = truncate_width(&name, budget);
                spans.push(Span::styled(tname, name_style));
                let pad = name_region_w.saturating_sub(left_w + nw);
                if pad > 0 {
                    spans.push(Span::from(" ".repeat(pad)));
                }
                let cells = app.detail_cells_get(&e.path);
                for (ci, id) in detail_cols.iter().enumerate() {
                    let w = fileops::detail_column_width(id).unwrap();
                    let cell = cells
                        .and_then(|c| c.get(ci))
                        .map(String::as_str)
                        .unwrap_or("");
                    let (cell_t, _) = truncate_width(cell, w);
                    spans.push(Span::from(format!("  {cell_t:>w$}")).dim());
                }
            } else if filtering && !query.is_empty() {
                // While filtering, highlight the matching part (the query portion).
                spans.extend(highlight_match(&name, &query, name_style));
            } else {
                spans.push(Span::styled(name, name_style));
            }

            let line = Line::from(spans);
            if i == app.tab.selected {
                line.reversed() // the selected row is reversed (including colored spans)
            } else {
                line
            }
        })
        .collect();

    let root = app.tab.root.clone();
    // The title border shows the path + (if a git repo) the branch name alongside it. While filtering,
    // also show the query's match count; while showing changes-only, show the change count.
    let title = match (app.filter_query(), app.git_branch()) {
        (Some(q), _) => format!(
            " {}  /{}  ({}) ",
            app.format_path(&root),
            q,
            app.tab.entries.len()
        ),
        (None, _) if app.changed_filter() => format!(
            " {}  ± {} ({}) ",
            app.format_path(&root),
            tr(app.lang, Msg::StChangedOnly),
            app.tab.entries.len()
        ),
        (None, Some(branch)) => format!(
            " {}  {} {} ",
            app.format_path(&root),
            icons::branch_marker(icons_on),
            branch
        ),
        (None, None) => format!(" {} ", app.format_path(&root)),
    };

    // Only `[offset..end]` has already been turned into Lines, so Paragraph's vertical scroll is 0
    // (rather than skipping past offset rows without drawing them, they were never created in the first place).
    let widget = Paragraph::new(lines).block(Block::bordered().title(title));
    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    // --- root title branch marker (⎇) gated by ui.icons -------------------------------------
    // U+2387 ALTERNATIVE KEY SYMBOL (⎇) is absent from Menlo/SF Mono/Monaco/Courier/Andale
    // Mono/Courier New/HackGen Console NF, so macOS always falls back to Apple Symbols — whose
    // glyph advance (~1.033em) is ~1.7x a Menlo cell (~0.602em). Before this fix the glyph was
    // emitted unconditionally regardless of `ui.icons`, so a terminal without a matching
    // symbol-font fallback would show tofu, and `ui.icons=false` could not suppress it either.
    #[cfg(feature = "git")]
    #[test]
    fn root_title_branch_marker_gated_by_icons() {
        let dir = unique_tmp("konoma_tree_title_branch_marker_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} 失敗");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t.t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["branch", "-M", "trunk"]);

        // icons=true (default): the branch glyph appears next to the branch name.
        let mut app = App::new(
            dir.canonicalize().unwrap(),
            crate::config::Config::default(),
        )
        .unwrap();
        app.refresh_git_if_needed();
        assert_eq!(app.git_branch(), Some("trunk"), "ブランチ名が取れていない");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 8)).unwrap();
        term.draw(|f| render(f, &mut app, f.area())).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            s.contains('⎇'),
            "icons=true でブランチアイコンが出ない: {s}"
        );
        assert!(s.contains("trunk"), "ブランチ名が出ない: {s}");

        // icons=false: the Unicode glyph never appears (would spill into the next cell on a
        // terminal without a matching symbol-font fallback); the ASCII label takes its place.
        let mut cfg = crate::config::Config::default();
        cfg.ui.icons = false;
        let mut app2 = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
        app2.refresh_git_if_needed();
        let mut term2 = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 8)).unwrap();
        term2.draw(|f| render(f, &mut app2, f.area())).unwrap();
        let s2: String = term2
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !s2.contains('⎇'),
            "icons=false なのにブランチアイコンが出ている: {s2}"
        );
        assert!(
            s2.contains("br:"),
            "icons=false で ASCII 代替(br:)が出ない: {s2}"
        );
        assert!(s2.contains("trunk"), "ブランチ名が出ない: {s2}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn highlight_match_marks_query_in_yellow() {
        let spans = highlight_match("src/main.rs", "rs", Style::default());
        // Joining the spans reconstructs the original string (no missing/duplicated content).
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "src/main.rs");
        // The span for the matched "rs" is highlighted yellow + bold.
        assert!(
            spans.iter().any(|s| s.content.as_ref() == "rs"
                && s.style.fg == Some(Color::Yellow)
                && s.style.add_modifier.contains(Modifier::BOLD)),
            "マッチ部が黄色強調でない: {spans:?}"
        );
    }

    #[test]
    fn highlight_match_empty_query_is_single_span() {
        let spans = highlight_match("file.txt", "", Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "file.txt");
    }

    #[test]
    fn highlight_match_non_ascii_marks_query() {
        // Highlighting still shows up for a non-ASCII name (no silent degradation).
        let spans = highlight_match("café.txt", "É", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "café.txt");
        assert!(
            spans.iter().any(|s| s.content.as_ref() == "é"
                && s.style.fg == Some(Color::Yellow)
                && s.style.add_modifier.contains(Modifier::BOLD)),
            "非 ASCII マッチ部が黄色強調でない: {spans:?}"
        );
    }

    #[test]
    fn highlight_match_lowercase_expands_no_panic() {
        // Even a combination where lowercasing can shift char boundaries doesn't panic, and joining restores the original.
        // 'İ' (U+0130) expands via to_lowercase into 2 characters: 'i' + a combining dot.
        let spans = highlight_match("İẞ", "ß", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "İẞ");
        // 'ẞ' (U+1E9E) matches lowercase 'ß' and gets highlighted.
        assert!(
            spans
                .iter()
                .any(|s| s.content.as_ref() == "ẞ" && s.style.fg == Some(Color::Yellow)),
            "強調が出ていない: {spans:?}"
        );
    }

    #[test]
    fn help_sections_lists_tree_keys_and_leaders() {
        let dir = unique_tmp("konoma_help_sections_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let app = App::new(dir.clone(), crate::config::Config::default()).unwrap();
        let secs = help_sections(&app);
        assert!(
            secs.len() >= 3,
            "ツリー/ファイル管理/選択など複数セクション"
        );
        // The first section is tree operations. It includes representative key rows.
        let tree = &secs[0];
        assert_eq!(tree.title, tr(app.lang, crate::i18n::Msg::TreeSection));
        assert!(
            tree.rows.iter().any(|(k, _)| k.contains("j / k")),
            "j/k 行が無い: {:?}",
            tree.rows
        );
        assert!(
            tree.rows.iter().any(|(k, _)| k == "o"),
            "git 変更ハブ(o)の行が無い"
        );
        // No section is empty (confirms the policy of never creating a title-only section).
        assert!(
            secs.iter().all(|s| !s.title.is_empty()),
            "空タイトルのセクションがある"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
