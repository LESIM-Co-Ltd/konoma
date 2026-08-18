// Status-line chrome (lightline-style). Convention (converged from htop/lazygit/k9s/yazi):
//   - Persistent context (mode/path/zoom/tab) = top (header)
//   - Key hints = bottom (footer). Always show the handful that matter most, in priority order,
//     as many as fit the width.
//   - A transient message (flash) or an in-progress code (prefix→…) temporarily occupies the
//     footer's position.
// Placement switches via the `ui.statusbar` setting (split = default / bottom). Here we just
// assemble the "content" (spans).

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, DisplayMode, InternalMode, KeyScheme, Mode, PathStyle};
use crate::i18n::{tr, Lang, Msg};
use crate::keymap::Surface;

/// Shared helper that builds one mode chip. Background color = meaning; foreground is black/white by lightness.
fn chip(lang: Lang, msg: Msg, bg: Color, dark_bg: bool) -> Span<'static> {
    chip_str(tr(lang, msg).to_string(), bg, dark_bg)
}

/// Truncates `s` to display width `max` (CJK-aware; appends `…` when cut). The context bar shares
/// width with the tab bar (`context_width` is used to size it), so labels here — like the linked-
/// worktree chip's origin repo name — must stay bounded rather than growing unboundedly with an
/// arbitrary directory name.
fn truncate_display(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
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
    out
}

/// A chip for arbitrary text (used for breadcrumbs like `GRAPH - BRANCH`).
fn chip_str(text: String, bg: Color, dark_bg: bool) -> Span<'static> {
    let fg = if dark_bg { Color::White } else { Color::Black };
    Span::styled(
        format!(" {text} "),
        Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
}

/// Display-mode (outer) chip. In a Git view = GIT (yellow) / TREE = white / PREVIEW/IMAGE = blue.
/// Since Git has grown, the changes hub/log/graph/branches/detail **promote the main mode to GIT**.
fn display_chip(app: &App) -> Span<'static> {
    if app.in_git_view() {
        // Which backend is answering has to be visible: a colocated repository has both, and a diff
        // taken against `@-` is not the same thing as one taken against HEAD.
        return match app.git_vcs {
            crate::vcs::VcsKind::Git => chip(app.lang, Msg::StGit, Color::Yellow, false),
            #[cfg(feature = "git")]
            crate::vcs::VcsKind::Jj => chip(app.lang, Msg::StJj, Color::Magenta, false),
        };
    }
    let (msg, bg, dark) = match app.display_mode() {
        DisplayMode::Tree => (Msg::StTree, Color::White, false),
        // Part of the "viewing content" blue family. IMAGE uses a different lightness (LightBlue)
        // to signal its distinct zoom/pan operations.
        DisplayMode::Preview => (Msg::StPreview, Color::Blue, true),
        DisplayMode::Image => (Msg::StImage, Color::LightBlue, false),
        // Table is also part of the "viewing content" blue family (a csvlens-style browsing mode).
        DisplayMode::Table => (Msg::StTable, Color::Cyan, true),
    };
    chip(app.lang, msg, bg, dark)
}

/// Internal-mode (inner) chip. Color = meaning (delete = red / create = green / select = magenta / input = cyan / auxiliary = yellow).
/// Since a Git view's main mode is GIT (above), the inner chip shows the concrete view name (CHANGES/LOG/GRAPH/BRANCH/…).
/// Nested state is a breadcrumb `parent - child` (e.g. the graph's branch panel = `GRAPH - BRANCH`).
fn internal_chip(app: &App) -> Option<Span<'static>> {
    // The graph's branch panel is a sub-state of GRAPH, so it's shown as a breadcrumb.
    if matches!(app.internal_mode()?, InternalMode::GitGraphPicker) {
        let text = format!(
            "{} - {}",
            tr(app.lang, Msg::StGraph),
            tr(app.lang, Msg::StBranch)
        );
        return Some(chip_str(text, Color::Yellow, false));
    }
    let (msg, bg, dark) = match app.internal_mode()? {
        // Help is part of the same "view-only overlay" family as Info (dim/DarkGray).
        InternalMode::Help => (Msg::StHelp, Color::DarkGray, true),
        InternalMode::Visual => (Msg::StVisual, Color::Magenta, true),
        // Preview selection: character range (v) = VISUAL / line (V) = V-LINE.
        InternalMode::PreviewVisual => {
            if app.preview_visual_linewise() {
                (Msg::StVisualLine, Color::Magenta, true)
            } else {
                (Msg::StVisual, Color::Magenta, true)
            }
        }
        InternalMode::Filter => (Msg::StFilter, Color::Yellow, false),
        // Show changed files only (Agent Watch). A list of git changes = part of the Git family's
        // yellow.
        InternalMode::ChangedFilter => (Msg::StChangedOnly, Color::Yellow, false),
        InternalMode::Search => (Msg::StSearch, Color::Yellow, false),
        InternalMode::Sort => (Msg::StSort, Color::Yellow, false),
        InternalMode::Mark => (Msg::StMark, Color::Yellow, false),
        InternalMode::Bookmarks => (Msg::StBookmarks, Color::Yellow, false),
        InternalMode::Tabs => (Msg::StTabs, Color::Yellow, false),
        InternalMode::Outline => (Msg::StOutline, Color::Yellow, false),
        InternalMode::Info => (Msg::StInfo, Color::DarkGray, true),
        // A "read-only" auxiliary overlay, so it's the same family as Info (DarkGray).
        InternalMode::TableCell => (Msg::StTableCell, Color::DarkGray, true),
        InternalMode::Create => (Msg::StCreate, Color::Green, false),
        InternalMode::Rename => (Msg::StRename, Color::Cyan, false),
        InternalMode::BatchRename => (Msg::StBatchRename, Color::Cyan, false),
        InternalMode::RenamePreview => (Msg::StRenameConfirm, Color::Cyan, false),
        InternalMode::DeleteConfirm => (Msg::StDelete, Color::Red, true),
        // A drag-and-drop transfer is non-destructive, so cyan (choosing copy/move).
        InternalMode::DropConfirm => (Msg::StDrop, Color::Cyan, false),
        // The app-quit confirmation is non-destructive = yellow.
        InternalMode::QuitConfirm => (Msg::StQuit, Color::Yellow, false),
        // The bookmark-overwrite confirmation is also non-destructive = yellow.
        InternalMode::BookmarkConfirm => (Msg::StMarkOverwrite, Color::Yellow, false),
        InternalMode::GitChanges => (Msg::StChanges, Color::Yellow, false),
        // diff is part of the "viewing content" preview family, so blue.
        InternalMode::GitDiff => (Msg::StDiff, Color::Blue, true),
        // A commit is "creating/finalizing", so green (same family as create).
        InternalMode::Commit => (Msg::StCommit, Color::Green, false),
        // log is history browsing (the Git family's yellow). Detail is part of the "viewing
        // content" diff family, so blue.
        InternalMode::GitLog => (Msg::StLog, Color::Yellow, false),
        InternalMode::GitDetail => (Msg::StCommitDiff, Color::Blue, true),
        // Branch operations are also part of the Git family's yellow.
        InternalMode::GitBranch => (Msg::StBranch, Color::Yellow, false),
        // Worktrees are also part of the Git family's yellow.
        InternalMode::GitWorktrees => (Msg::StWorktrees, Color::Yellow, false),
        InternalMode::GitGraph => (Msg::StGraph, Color::Yellow, false),
        // GitGraphPicker is already handled as a breadcrumb up top (early return) = it never
        // reaches here. Even if that guard ever slips, fall back gently to the GRAPH chip so
        // nothing panics inside the render loop.
        InternalMode::GitGraphPicker => (Msg::StGraph, Color::Yellow, false),
    };
    Some(chip(app.lang, msg, bg, dark))
}

/// Shared helper that formats one hint as `"key:label"`. Used by each view's `footer_hints`.
pub fn hint(lang: Lang, keys: &str, msg: Msg) -> String {
    format!("{keys}:{}", tr(lang, msg))
}

/// Page-step hint (notation changes by scheme). Shared hint used by both Tree and Preview.
pub fn page_hint(app: &App) -> String {
    match app.key_scheme {
        KeyScheme::Vim => hint(app.lang, "^f/^b", crate::i18n::Msg::HintPage),
        KeyScheme::Less => hint(app.lang, "Spc/b", crate::i18n::Msg::HintPage),
    }
}

/// Page-step description for the `?` help (wording changes by scheme). Shared string used by Tree/Preview help.
pub fn page_help(app: &App) -> String {
    match app.key_scheme {
        KeyScheme::Vim => tr(app.lang, crate::i18n::Msg::StScrollHintCtrl),
        KeyScheme::Less => tr(app.lang, crate::i18n::Msg::StPagerSpaceHint),
    }
    .to_string()
}

/// Persistent context line: **each view owns its own context spans (chips, etc.)**; here we
/// delegate to that and then append the common path display style. (Tree = chip only / Preview = chip + image zoom.)
pub fn context_spans(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    // Background-work indicator (only while something is running; `ui.busy_indicator`). Spinner +
    // the first job's label (+n when several are running). Shows nothing when there are no jobs =
    // the idle look stays as before.
    if app.busy_indicator_active() {
        let jobs = app.busy_jobs();
        if let Some(first) = jobs.first() {
            let label = tr(app.lang, *first).to_string();
            // A file operation also appends progress (N/M, plus the leaf-file count for large
            // directories).
            let label = match app.fileop_progress_text() {
                Some(d) if *first == Msg::BusyFileOp => format!("{label} {d}"),
                _ => label,
            };
            let txt = if jobs.len() > 1 {
                format!("{} {} +{}  ", app.spinner_glyph(), label, jobs.len() - 1)
            } else {
                format!("{} {}  ", app.spinner_glyph(), label)
            };
            spans.push(Span::from(txt).dim());
        }
    }
    // Outer chip (display mode) + inner chip (internal mode, if any).
    spans.push(display_chip(app));
    if let Some(ic) = internal_chip(app) {
        spans.push(Span::from(" "));
        spans.push(ic);
    }
    // The listing on screen is out of date (the last rebuild failed and the previous snapshot is
    // still being shown). Like FOLLOW below, this coexists with every mode, so it gets its own
    // chip — and unlike the flash that announces the moment it happens, a chip stays up for as
    // long as it is true. That is the whole point here: `rebuild_tree` deliberately keeps the last
    // good listing, so a stale tree is indistinguishable from a healthy one, and a message that
    // the next keypress wipes leaves nothing behind to doubt it by. Placed ahead of FOLLOW/WT so a
    // warning about what you are reading sits next to the mode chips instead of being pushed
    // around by unrelated indicators. Yellow = the warning family (the same as the
    // overwrite/quit confirmations); red stays reserved for destructive actions.
    if app.tree_stale() {
        spans.push(Span::from(" "));
        spans.push(chip(app.lang, Msg::StListingStale, Color::Yellow, false));
    }
    // Follow mode (`F`) coexists with other states, so it always shows as its own chip (so ON is
    // obvious at a glance).
    if app.follow_enabled() {
        spans.push(Span::from(" "));
        spans.push(chip(app.lang, Msg::StFollow, Color::Green, false));
    }
    // Linked-worktree indicator: shown **only** while inside a linked worktree (never the main
    // working tree), so — like FOLLOW above — it coexists with any other state and always appears
    // as its own chip. Without this, the tree/preview title's root-directory name is the sole clue
    // to "where am I", and for a linked worktree that name is an unrelated branch/feature name, not
    // the project's — there is nowhere else on screen that says "this is a worktree" or names the
    // origin repo. `app.worktree_origin()` reads a value cached alongside `git_branch` (refreshed
    // once per root change), never git itself, so this costs nothing extra per frame.
    if let Some(origin) = app.worktree_origin() {
        spans.push(Span::from(" "));
        spans.push(chip(app.lang, Msg::StWorktreeChip, Color::Cyan, false));
        spans.push(Span::from(format!(" {}", truncate_display(origin, 20))).dim());
    }
    // Each view's own addition (Tree = sort/selection count / Preview = image zoom). No chip is
    // shown here.
    spans.extend(match app.tab.mode {
        Mode::Tree => crate::ui::tree::context(app),
        Mode::Preview => crate::ui::preview::context(app),
    });
    let lang = app.lang;
    let style = match app.path_style {
        PathStyle::Relative => tr(lang, crate::i18n::Msg::RelLabel),
        PathStyle::Home => "~",
        PathStyle::Full => tr(lang, crate::i18n::Msg::StPathAbs),
    };
    let prefix = tr(lang, crate::i18n::Msg::PathLabel);
    spans.push(Span::from(format!("  {prefix}{style}")).dim());
    spans
}

/// Per-internal-mode footer help (the bottom footer is the source of truth for "keys you can press now").
/// Dialog/visual keys are shown here (the delete confirmation's y/!/n also appears in the box, but the source of truth is here).
fn mode_footer(app: &App) -> Option<Vec<Span<'static>>> {
    let lang = app.lang;
    let s = match app.internal_mode()? {
        // Help's own operation keys (j/k/g/G scroll, q/Esc close). If we kept showing hints from
        // the surface underneath (Tree/Preview), it would advertise keys you can't press while
        // help is showing (the centered popup doesn't cover the top/bottom edges, so the footer
        // is always visible).
        InternalMode::Help => tr(lang, crate::i18n::Msg::StHelpHint),
        InternalMode::DeleteConfirm if app.confirm_is_branch_delete() => {
            tr(lang, crate::i18n::Msg::StDeleteForce)
        }
        InternalMode::DeleteConfirm => tr(lang, crate::i18n::Msg::StTrashDelete),
        InternalMode::DropConfirm => tr(lang, crate::i18n::Msg::StCopyMoveHint),
        InternalMode::QuitConfirm => tr(lang, crate::i18n::Msg::StQuitHint),
        InternalMode::BookmarkConfirm => tr(lang, crate::i18n::Msg::StMarkOverwriteHint),
        InternalMode::RenamePreview => tr(lang, crate::i18n::Msg::StApply),
        InternalMode::Create => tr(lang, crate::i18n::Msg::StCreateHint),
        InternalMode::BatchRename => tr(lang, crate::i18n::Msg::StRenamePreviewHint),
        InternalMode::Rename => tr(lang, crate::i18n::Msg::StRenameHint),
        InternalMode::Visual => tr(lang, crate::i18n::Msg::VisualOpsHint),
        InternalMode::PreviewVisual => {
            if app.preview_visual_linewise() {
                tr(lang, crate::i18n::Msg::PreviewVisualLineHint)
            } else {
                tr(lang, crate::i18n::Msg::PreviewVisualHint)
            }
        }
        InternalMode::Info => tr(lang, crate::i18n::Msg::StCloseHint),
        InternalMode::TableCell => tr(lang, crate::i18n::Msg::TableCellActions),
        InternalMode::ChangedFilter => tr(lang, crate::i18n::Msg::ChangedFilterHint),
        InternalMode::GitChanges => tr(lang, crate::i18n::Msg::StGitHubKeys),
        InternalMode::GitGraph => tr(lang, crate::i18n::Msg::GitNavDetailCommitHint),
        InternalMode::GitGraphPicker => tr(lang, crate::i18n::Msg::GraphPickerFooter),
        InternalMode::GitBranch => tr(lang, crate::i18n::Msg::BranchesNavHint),
        InternalMode::GitWorktrees => tr(lang, crate::i18n::Msg::WorktreesNavHint),
        InternalMode::GitDiff => tr(lang, crate::i18n::Msg::DiffScrollDiscardHint),
        InternalMode::Commit => tr(lang, crate::i18n::Msg::StCommitHint),
        InternalMode::GitLog => tr(lang, crate::i18n::Msg::GitNavDetailHint),
        InternalMode::GitDetail => tr(lang, crate::i18n::Msg::DiffScrollHint),
        // Filter/search/sort/mark/bookmarks are handled by their dedicated prompt below.
        _ => return None,
    };
    Some(vec![Span::from(s).bold()])
}

/// Footer: flash → which-key leader → in-progress input guide → priority-ordered key hints (as many as fit in `width`).
pub fn footer_spans(app: &App, width: u16) -> Vec<Span<'static>> {
    let lang = app.lang;
    if let Some(msg) = &app.flash {
        return vec![Span::from(msg.clone()).bold()];
    }
    // which-key: after pressing a leader (Space = file management / y = path copy), show the
    // candidates for the next keystroke.
    if let Some(spans) = whichkey_spans(app) {
        return spans;
    }
    // While branch-filter input is active, show the query prompt (takes priority over the mode
    // footer).
    if app.git_branch_filtering() {
        let q = app.git_branch_query();
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!(
                "  {}",
                tr(lang, crate::i18n::Msg::StApplyClearHint)
            ))
            .dim(),
        ];
    }
    // Same treatment for the worktree list's own filter input.
    if app.git_worktree_filtering() {
        let q = app.git_worktree_query();
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!(
                "  {}",
                tr(lang, crate::i18n::Msg::StApplyClearHint)
            ))
            .dim(),
        ];
    }
    // While a dialog/visual mode is active, show that mode's keys (the bottom footer is the source
    // of truth).
    if let Some(spans) = mode_footer(app) {
        return spans;
    }
    // While filter input is active, show the query prompt in the footer ("/<query>▏").
    if app.is_filtering() {
        let q = app.filter_query().unwrap_or("");
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!(
                "  {}",
                tr(lang, crate::i18n::Msg::StApplyClearHint)
            ))
            .dim(),
        ];
    }
    // Same treatment while in-preview search input is active: show the prompt.
    if app.is_searching() {
        let q = app.search_input().unwrap_or("");
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!("  {}", tr(lang, crate::i18n::Msg::StSearchHint))).dim(),
        ];
    }
    // While the sort selection menu is showing, show the choices in the footer (current value ▸
    // key list).
    if app.is_sort_menu() {
        return vec![
            Span::from(format!("{} ▸ ", app.sort_label())).bold(),
            Span::from(tr(lang, crate::i18n::Msg::StSortHint)).dim(),
        ];
    }
    // The bookmark-pending-registration prompt (m = register; '= list is on the
    // Surface::Bookmarks side).
    if app.is_marking() {
        return vec![Span::from(tr(lang, crate::i18n::Msg::MarkHint)).bold()];
    }
    vec![Span::from(fit_tokens(&hint_tokens(app), width)).dim()]
}

/// which-key popup (footer-line version). Only while waiting on a leader, shows the menu heading + candidates
/// (`key:label`). Returns None if pending_leader is None. Zero per-frame cost (only while waiting).
fn whichkey_spans(app: &App) -> Option<Vec<Span<'static>>> {
    let lead = app.pending_leader?;
    let menu = app.keymaps.leaders.get(&lead)?;
    let title = tr(app.lang, menu.title);
    // The code-block copy entry only makes sense while a Markdown code block is focused; hide it
    // everywhere else so the normal path-copy menu stays uncluttered.
    let show_code = app.surface() == Surface::PreviewText && app.md_focused_code();
    let items: Vec<String> = menu
        .items
        .iter()
        .filter(|it| show_code || !matches!(it.action, crate::keymap::Action::CopyCodeBlock))
        .map(|it| {
            let k = match it.key.code {
                KeyCode::Char(' ') => "Space".to_string(),
                KeyCode::Char(c) => c.to_string(),
                _ => "?".to_string(),
            };
            let label = tr(app.lang, it.label);
            format!("{k}:{label}")
        })
        .collect();
    Some(vec![
        Span::from(format!("{title} ▸ ")).bold(),
        Span::from(items.join("  ")).dim(),
    ])
}

/// Operation hints are **owned by each view**. Here we just delegate to the active view.
/// (Tree = `ui::tree::footer_hints` / Preview = `ui::preview::footer_hints`. Kind differences are absorbed inside preview.)
fn hint_tokens(app: &App) -> Vec<String> {
    match app.tab.mode {
        Mode::Tree => crate::ui::tree::footer_hints(app),
        Mode::Preview => crate::ui::preview::footer_hints(app),
    }
}

/// Join tokens with the separator `  ` and return only as many as fit in display width `max`.
/// If cut off partway (not all fit), append ` …` at the end to indicate there is more.
fn fit_tokens(tokens: &[String], max: u16) -> String {
    const SEP: &str = "  ";
    let max = max as usize;
    let ellipsis = " …";
    let mut out = String::new();
    let mut used = 0usize;
    let mut shown = 0usize;
    for tok in tokens {
        let add = if out.is_empty() {
            tok.width()
        } else {
            SEP.width() + tok.width()
        };
        // If tokens remain, reserve room for the ellipsis and check whether it still fits.
        let reserve = if shown + 1 < tokens.len() {
            ellipsis.width()
        } else {
            0
        };
        if used + add + reserve > max {
            break;
        }
        if !out.is_empty() {
            out.push_str(SEP);
        }
        out.push_str(tok);
        used += add;
        shown += 1;
    }
    if shown < tokens.len() && shown > 0 {
        out.push_str(ellipsis);
    }
    out
}

/// Render everything on one line (for statusbar="bottom"/"top"). Concatenates context + footer.
pub fn render_combined(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = context_spans(app);
    let used: usize = spans.iter().map(|s| s.width()).sum();
    spans.push(Span::from("  "));
    let rest = area.width.saturating_sub(used as u16 + 2);
    spans.extend(footer_spans(app, rest));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the context only (for the right side of the split header). Right-aligned.
/// Display width of the top-right context spans (used to shrink the tab bar's share of the row).
pub fn context_width(app: &App) -> u16 {
    context_spans(app)
        .iter()
        .map(|s| s.content.as_ref().width() as u16)
        .sum()
}

pub fn render_context(frame: &mut Frame, app: &App, area: Rect) {
    let p =
        Paragraph::new(Line::from(context_spans(app))).alignment(ratatui::layout::Alignment::Right);
    frame.render_widget(p, area);
}

/// Render the footer only (key hints, etc.) (for the bottom row of the split).
pub fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(footer_spans(app, area.width))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::unique_tmp;

    #[test]
    fn markdown_preview_footer_shows_link_keys() {
        // The Markdown preview footer shows link operations (Tab = focus / ↵ = open).
        // Plain text/code doesn't show them (there are no links).
        let dir = unique_tmp("konoma_md_hints_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), b"[x](https://e.com)\n").unwrap();
        std::fs::write(dir.join("b.txt"), b"plain text\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

        // Preview a.md → link operations present.
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("a.md"))
            .unwrap();
        app.tree_activate().unwrap();
        let md = hint_tokens(&app).join(" ");
        assert!(md.contains("Tab:link"), "md にリンク操作が無い: {md}");
        assert!(md.contains("↵:open"), "md に開く操作が無い: {md}");

        // Preview b.txt → no link operations (the normal text hints).
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("b.txt"))
            .unwrap();
        app.tree_activate().unwrap();
        let txt = hint_tokens(&app).join(" ");
        assert!(
            !txt.contains("Tab:link") && !txt.contains("↵:open"),
            "テキストにリンク操作が出ている: {txt}"
        );
        assert!(txt.contains("hl:"), "テキストは横移動ヒントを出す: {txt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The footer is a single shared line, so every hint label has to be a short word. `v`/`V` used
    /// to be wired to `Msg::PreviewSelectHelp` — the explanatory sentence written for the `?` help
    /// screen ("v: select a character range   V: select whole lines (copy with y)"). That one hint
    /// took 60+ columns and pushed `Y:@ref` / `F:FOLLOW` and everything after it past the trailing
    /// `…`, and it repeated the key prefix (`v/V:` immediately followed by another `v:`/`V:`).
    /// Checked in both languages: the short label shows up, the explanatory wording does not.
    #[test]
    fn windowed_preview_footer_uses_the_short_select_label() {
        let dir = unique_tmp("konoma_footer_select_label_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello\nworld\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("a.txt"))
            .unwrap();
        app.tree_activate().unwrap();
        // The v/V hint only appears for a windowed preview (Code/Text/raw Markdown); if this ever
        // stops holding, the assertions below would pass vacuously.
        assert!(app.is_windowed(), "テキストは windowed プレビューのはず");

        for (lang, short, long) in [
            (Lang::En, "v/V:select", "select a character range"),
            (Lang::Jp, "v/V:選択", "文字範囲を選択"),
        ] {
            app.lang = lang;
            let footer = hint_tokens(&app).join("  ");
            assert!(
                footer.contains(short),
                "{lang:?}: 短いフッター語 {short:?} が無い: {footer}"
            );
            assert!(
                !footer.contains(long),
                "{lang:?}: ヘルプ用の長文 {long:?} がフッターに出ている: {footer}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The counterpart to the test above: the long explanatory wording must stay in the `?` help
    /// screen, which is where it belongs. Without this, "shorten the footer" could be satisfied by
    /// deleting the explanation outright — losing the only place that spells out what `v` and `V`
    /// each do.
    #[test]
    fn preview_help_screen_keeps_the_long_select_explanation() {
        let dir = unique_tmp("konoma_help_select_help_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello\nworld\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("a.txt"))
            .unwrap();
        app.tree_activate().unwrap();

        for (lang, long) in [
            (Lang::En, "select a character range"),
            (Lang::Jp, "文字範囲を選択"),
        ] {
            app.lang = lang;
            // Go through the real `?` overlay body, not just `help_sections`, so this covers what
            // the user actually reads.
            let help: String = crate::ui::help::help_lines(&app)
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
                .join("");
            assert!(
                help.contains(long),
                "{lang:?}: ヘルプ画面から説明文 {long:?} が消えている: {help}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Same-shaped hole, closed by type rather than one case at a time: sweep both footers (Tree and
    /// every Preview kind reachable here) in both languages and require every token to stay within a
    /// short display width. A help-screen sentence wired into a footer trips this no matter which
    /// `Msg` it is, so the next one gets caught here instead of on screen.
    #[test]
    fn no_footer_hint_is_long_enough_to_be_help_text() {
        // The widest legitimate token today is ~20 columns ("C-n/p:next/prev file"); the sentence
        // this guards against was 69. 24 leaves room for wording tweaks without letting prose in.
        const MAX_COLS: usize = 24;
        let dir = unique_tmp("konoma_footer_token_width_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello\nworld\n").unwrap();
        std::fs::write(
            dir.join("b.md"),
            b"# t\n\n- [ ] task\n\n[x](https://e.com)\n",
        )
        .unwrap();
        std::fs::write(dir.join("c.csv"), b"a,b\n1,2\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

        let check = |app: &App, where_: &str| {
            for tok in hint_tokens(app) {
                assert!(
                    tok.width() <= MAX_COLS,
                    "{where_}: フッターのヒントが長すぎる({} 桁 > {MAX_COLS}) — ヘルプ用の長文を渡していないか: {tok:?}",
                    tok.width()
                );
            }
        };

        for lang in [Lang::En, Lang::Jp] {
            app.lang = lang;
            // Tree footer.
            check(&app, &format!("{lang:?}/tree"));
            // Each preview kind's footer (text = windowed, Markdown = decorated, CSV = table).
            for name in ["a.txt", "b.md", "c.csv"] {
                app.back_to_tree();
                app.tab.selected = app
                    .tab
                    .entries
                    .iter()
                    .position(|e| e.path.ends_with(name))
                    .unwrap();
                app.tree_activate().unwrap();
                check(&app, &format!("{lang:?}/{name}"));
            }
            app.back_to_tree();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fit_tokens_priority_and_truncation() {
        let toks: Vec<String> = ["aaa", "bbb", "ccc", "ddd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Plenty wide → everything, no ellipsis.
        let all = fit_tokens(&toks, 80);
        assert_eq!(all, "aaa  bbb  ccc  ddd");
        // Narrow → keep as many as fit, prioritizing the front, plus " …".
        let some = fit_tokens(&toks, 12);
        assert!(some.starts_with("aaa"), "先頭優先: {some}");
        assert!(some.ends_with('…'), "続きありの省略記号: {some}");
        assert!(!some.contains("ddd"), "末尾は落ちる: {some}");
        // Does not panic even at width 0.
        assert_eq!(fit_tokens(&toks, 0), "");
    }

    /// `truncate_display` (used by the linked-worktree chip to bound an arbitrary origin directory
    /// name) is a no-op under the limit, truncates with a trailing `…` over it, is CJK-width-aware
    /// (a 2-column glyph counts as 2), and doesn't panic at `max == 0`.
    #[test]
    fn truncate_display_truncates_long_names() {
        assert_eq!(truncate_display("short", 20), "short", "上限以下はそのまま");
        let long = "a".repeat(30);
        let t = truncate_display(&long, 20);
        assert_eq!(t.width(), 20, "上限ちょうどの表示幅に収める");
        assert!(t.ends_with('…'), "切った印に … を付ける");
        // CJK: each glyph is display-width 2, so 20 columns holds far fewer than 20 characters.
        let cjk = "名".repeat(30);
        let tc = truncate_display(&cjk, 20);
        assert!(tc.width() <= 20, "CJK でも表示幅の上限を超えない: {tc:?}");
        assert!(tc.ends_with('…'));
        assert_eq!(truncate_display("x", 0), "", "上限0でもパニックしない");
    }

    #[test]
    fn footer_shows_whichkey_menu_while_leader_pending() {
        let dir = unique_tmp("konoma_whichkey_footer_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        // No leader pressed yet: which-key does not show (the footer shows normal hints).
        assert!(whichkey_spans(&app).is_none(), "未押下では None");

        // Equivalent to pressing the path-copy leader (y): set pending_leader → which-key heading +
        // candidates.
        app.pending_leader = Some(crate::keymap::LeaderId::Copy);
        let spans = footer_spans(&app, 120);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('▸'), "which-key 見出しが無い: {text}");
        assert!(text.contains(':'), "key:label 形式の候補が無い: {text}");
        // Calling whichkey_spans directly also gives Some (heading + candidates, 2 spans).
        let direct = whichkey_spans(&app).expect("リーダー中は Some");
        assert_eq!(direct.len(), 2, "見出し span + 候補 span");

        // If a flash is present, it takes priority over which-key (confirms footer_spans'
        // branching).
        app.flash = Some("hello flash".into());
        let f: String = footer_spans(&app, 120)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            f.contains("hello flash") && !f.contains('▸'),
            "flash 優先: {f}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The "WT <origin>" chip appears **only** while inside a linked worktree (`git worktree add`),
    /// never for the main working tree — the asymmetry is itself the signal. `origin` reads
    /// `App::worktree_origin()`, which is populated by the background status scan alongside
    /// `git_branch`, so this also exercises the real refresh path rather than poking the field directly.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_chip_shows_only_inside_a_linked_worktree() {
        // The leaf names stay short and fixed (<= the chip's 20-char truncation budget) so this
        // test asserts the *exact* origin name; `truncate_display_truncates_long_names` below
        // covers the truncation itself. Only the *parent* segment is uniqued, keeping the full
        // path collision-free across concurrent test runs without touching the basename the chip
        // actually renders (same reasoning as `app::tests::worktree_chip_re_verifies_across_tab_
        // switch_into_preview`, which does the same for the same reason).
        let base = unique_tmp("konoma_wt_chip");
        let dir = base.join("main");
        let linked = base.join("linked");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&linked);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        {
            let mut c = repo.config().unwrap();
            c.set_str("user.name", "T").unwrap();
            c.set_str("user.email", "t@t").unwrap();
            c.set_str("commit.gpgsign", "false").ok();
        }
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        let git = |cwd: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "init"]);
        let main_root = dir.canonicalize().unwrap();

        // Main working tree: no chip at all.
        let mut app_main = App::new(main_root.clone(), Config::default()).unwrap();
        app_main.refresh_git_if_needed();
        assert_eq!(
            app_main.worktree_origin(),
            None,
            "メインの作業ツリーでは worktree_origin が None"
        );
        let main_text: String = context_spans(&app_main)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            !main_text.contains("WT"),
            "メインの作業ツリーでは WT チップが出てはいけない: {main_text}"
        );

        git(
            &main_root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                linked.to_str().unwrap(),
            ],
        );
        let linked_root = linked.canonicalize().unwrap();
        let expected_origin = main_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();

        // Linked worktree: the chip + the origin repo's name.
        let mut app_wt = App::new(linked_root, Config::default()).unwrap();
        app_wt.refresh_git_if_needed();
        assert_eq!(
            app_wt.worktree_origin(),
            Some(expected_origin.as_str()),
            "linked worktree では元の repo 名が取れる"
        );
        let wt_text: String = context_spans(&app_wt)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            wt_text.contains("WT"),
            "linked worktree では WT チップが出るはず: {wt_text}"
        );
        assert!(
            wt_text.contains(&expected_origin),
            "元の repo 名 ({expected_origin}) が出ていない: {wt_text}"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
