// Rendering for the Git view (changes hub). The full-screen changed-file list opened with `o`.
// Each row: a staged indicator (●/blank) + a status marker (color = meaning) + the path relative to the repo.
// The selected row is reversed. Scrolls so the selection is always visible.
//
// The list data is held by App.git_view_entries() (rebuilt after open/refresh/write).
// Diff display and log are separate milestones (phase 3/5). This is just the list and staged display.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::vcs_color;

/// How the hub names the system answering it. git calls its pointer a branch and marks it with a
/// glyph; jj's label already opens with `@`, so it needs no marker of its own.
use crate::app::list_filter::ListId;
use crate::app::App;
use crate::i18n::tr;
use crate::ui::icons;

fn graph_style(style: konoma_vcs_core::style::GraphStyle) -> Style {
    use konoma_vcs_core::style::GraphStyle;
    use ratatui::style::Modifier;
    let GraphStyle { fg, bold } = style;
    let style = match fg {
        Some(color) => Style::new().fg(vcs_color(color)),
        None => Style::new(),
    };
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn repo_label(app: &App, icons_on: bool) -> Option<String> {
    let name = match app.git_vcs {
        crate::vcs::VcsKind::Git => "Git",
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => "jj",
    };
    let b = app.git_branch()?;
    Some(match icons::chip_marker(app.git_vcs, icons_on) {
        "" => format!("{name} {b}"),
        marker => format!("{name} {marker} {b}"),
    })
}

/// The system's name on its own, for a title with nothing to point at.
fn repo_name(app: &App) -> &'static str {
    match app.git_vcs {
        crate::vcs::VcsKind::Git => "Git",
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => "jj",
    }
}

/// Render the Git view's change list full-screen, in the content area instead of tree/preview.
pub fn render_changes(frame: &mut Frame, app: &App, area: Rect) {
    let lang = app.lang;
    let icons_on = app.cfg.ui.icons;
    let entries = app.git_view_entries();
    let sel = app.git_view_sel();
    // The `/` filter narrows the rows that are drawn; `shown` indexes all of `entries`.
    let shown = app.list_visible(ListId::Changes);
    let is_shown = |i: usize| shown.as_ref().is_none_or(|v| v.binary_search(&i).is_ok());
    let count = shown.as_ref().map_or(entries.len(), Vec::len);
    let q = app.list_filter_query(ListId::Changes);

    // Title: " Git ⎇ <branch>  (N) " (icons=false: " Git br: <branch>  (N) "). Just the glyph/label
    // when there's no branch. A query shows as " /<query>" before the count.
    let q_tag = if q.is_empty() {
        String::new()
    } else {
        format!(" /{q}")
    };
    let title = match repo_label(app, icons_on) {
        Some(l) => format!(" {l}{q_tag}  ({count}) "),
        None => format!(" {}{q_tag}  ({count}) ", repo_name(app)),
    };

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoChangesItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .filter(|(i, _)| is_shown(*i))
            .map(|(i, e)| {
                let st = e.status;
                // Staged indicator: ●=staged / blank=unstaged.
                let staged = if e.staged { "● " } else { "  " };
                // Status marker (color = meaning).
                let marker = Span::styled(
                    format!("{} ", st.marker()),
                    Style::new().fg(vcs_color(st.color())),
                );
                // Path relative to the repo (follows the display style via format_path).
                let name = app.format_path(&e.path);
                let line = Line::from(vec![
                    Span::from(staged.to_string()),
                    marker,
                    Span::from(name),
                ]);
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };

    // Decide the vertical scroll amount so the selected row is always visible (same naive follow as tree).
    let visible = area.height.saturating_sub(2) as usize;
    let sel = position_among(&shown, sel);
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };

    let widget = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .scroll((offset, 0));
    frame.render_widget(widget, area);
}

/// The position of row `sel` among the rows a filter leaves shown (`None` = no filter: the row
/// itself), for scrolling the cursor into view.
fn position_among(shown: &Option<Vec<usize>>, sel: usize) -> usize {
    match shown {
        Some(v) => v.iter().position(|&i| i == sel).unwrap_or(0),
        None => sel,
    }
}

/// Truncate styled spans to display width `max` (drop the tail and append `…` when over). Colors/decorations are preserved.
fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    let total: usize = spans.iter().map(|s| s.width()).sum();
    if total <= max {
        return spans;
    }
    if max == 0 {
        return Vec::new();
    }
    let budget = max - 1; // leave room for the trailing `…` (width 1)
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    'outer: for sp in spans {
        let style = sp.style;
        let mut cur = String::new();
        for ch in sp.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w > budget {
                if !cur.is_empty() {
                    out.push(Span::styled(cur, style));
                }
                out.push(Span::from("…"));
                break 'outer;
            }
            cur.push(ch);
            used += w;
        }
        if !cur.is_empty() {
            out.push(Span::styled(cur, style));
        }
    }
    out
}

/// Return one line of spans with dim meta info **aligned** at the right edge (right-aligned column) of the `left` spans.
/// - When it fits, pad with spaces out to the right edge (dates/authors line up neatly in a column).
/// - When it overflows, ellipsize the left side (subject etc.) and keep the meta at the right edge.
/// - When too narrow to even fit the meta, drop the meta and keep only the left side (ellipsized) (shed non-essential columns when narrow).
fn line_with_right_meta(
    left: Vec<Span<'static>>,
    meta: String,
    width: usize,
) -> Vec<Span<'static>> {
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr;
    if meta.is_empty() || width == 0 {
        return truncate_spans(left, width);
    }
    let meta_w = UnicodeWidthStr::width(meta.as_str());
    // Too narrow to even fit meta + a minimum 1 space → give up on meta, show only the left side.
    if meta_w + 2 > width {
        return truncate_spans(left, width);
    }
    let left_budget = width - meta_w - 1; // guarantee at least 1 space
    let mut out = truncate_spans(left, left_budget);
    let lw: usize = out.iter().map(|s| s.width()).sum();
    let pad = width.saturating_sub(lw + meta_w).max(1);
    out.push(Span::from(" ".repeat(pad)));
    out.push(Span::styled(meta, Style::new().fg(Color::DarkGray)));
    out
}

/// Render git log as a full-screen list. One row = short hash (dim) + summary + meta (author, date).
/// `ui.commit_meta_align` switches the meta between a right-aligned column (default) and right after the subject (inline).
/// The selected row is reversed. Scrolls vertically to keep the selection visible (same naive follow as tree/changes).
pub fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let lang = app.lang;
    let icons_on = app.cfg.ui.icons;
    let entries = app.git_log_entries();
    let sel = app.git_log_sel();
    let shown = app.list_visible(ListId::Log);
    let is_shown = |i: usize| shown.as_ref().is_none_or(|v| v.binary_search(&i).is_ok());
    let count = shown.as_ref().map_or(entries.len(), Vec::len);
    let q = app.list_filter_query(ListId::Log);
    let q_tag = if q.is_empty() {
        String::new()
    } else {
        format!(" /{q}")
    };

    let title = match app.git_branch() {
        Some(b) => match icons::chip_marker(app.git_vcs, icons_on) {
            "" => format!(" {} log {b}{q_tag}  ({count}) ", repo_name(app)),
            m => format!(" {} log {m} {b}{q_tag}  ({count}) ", repo_name(app)),
        },
        None => format!(" {} log{q_tag}  ({count}) ", repo_name(app)),
    };

    // How meta (author, date) is aligned. "inline"=right after the subject / anything else (default)=a right-aligned column.
    let align_right = app.cfg.ui.commit_meta_align != "inline";
    let inner_w = area.width.saturating_sub(2) as usize; // body width, excluding the 2-column border

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoCommitsItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .filter(|(i, _)| is_shown(*i))
            .map(|(i, c)| {
                // time_epoch is i64. Negative (before 1970) is clamped to 0 before passing to the u64 formatter.
                let date = crate::fileops::format_epoch_short(c.time_epoch.max(0) as u64);
                let line = if align_right {
                    // The short hash (a fixed anchor on the left) + summary on the left, author/date in a right-aligned column.
                    let left = vec![
                        Span::from(format!("{} ", c.short)).dim(),
                        Span::from(c.summary.clone()),
                    ];
                    let meta = format!("{} · {date}", c.author);
                    Line::from(line_with_right_meta(left, meta, inner_w))
                } else {
                    Line::from(vec![
                        Span::from(format!("{} ", c.short)).dim(),
                        Span::from(c.summary.clone()),
                        Span::from(format!("  · {}  · {date}", c.author)).dim(),
                    ])
                };
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };

    let visible = area.height.saturating_sub(2) as usize;
    let sel = position_among(&shown, sel);
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };

    let widget = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .scroll((offset, 0));
    frame.render_widget(widget, area);
}

/// Render the branch list as a full-screen list. One row = current marker (`* ` current / blank) + branch name.
/// The current branch is green + bold. The selected row is reversed. Scrolls vertically to keep the selection visible.
pub fn render_branches(frame: &mut Frame, app: &App, area: Rect) {
    use ratatui::style::Color;
    let lang = app.lang;
    let entries = app.git_branch_view(); // the display list after filtering
    let sel = app.git_branch_sel();
    // While filtering / with a query, show "/<query>" in the title.
    let q = app.git_branch_query();
    // git calls them branches, jj calls them bookmarks, and the difference matters: a bookmark does
    // not follow the working copy the way a branch follows HEAD.
    let kind = match app.git_vcs {
        crate::vcs::VcsKind::Git => "Git branches",
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => "jj bookmarks",
    };
    let title = if q.is_empty() {
        format!(" {kind}  ({}) ", entries.len())
    } else {
        format!(" {kind}  /{q}  ({}) ", entries.len())
    };

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoBranchesItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let mark = if b.is_current { "* " } else { "  " };
                let name = if b.is_current {
                    Span::styled(b.name.clone(), Style::new().fg(Color::Green).bold())
                } else {
                    Span::from(b.name.clone())
                };
                let line = Line::from(vec![Span::from(mark.to_string()).fg(Color::Green), name]);
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };

    let visible = area.height.saturating_sub(2) as usize;
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };
    let widget = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .scroll((offset, 0));
    frame.render_widget(widget, area);
}

/// Render the linked-worktree list (`w` from the changes hub) as a full-screen list.
/// One row = current marker (`* ` current / blank) + branch name (or `(detached)`/`(bare)`) +
/// short hash + path, with `main`/`bare`/`locked: <reason>`/`prunable` shown dim at the end where
/// they apply. The current worktree's branch name is green + bold (same convention as
/// `render_branches`). The selected row is reversed. Scrolls vertically to keep the selection visible.
pub fn render_worktrees(frame: &mut Frame, app: &App, area: Rect) {
    use ratatui::style::Color;
    let lang = app.lang;
    let entries = app.git_worktree_view(); // the display list after filtering
    let sel = app.git_worktree_sel();
    // While filtering / with a query, show "/<query>" in the title (same convention as branches).
    let q = app.git_worktree_query();
    let title = if q.is_empty() {
        format!(" Git worktrees  ({}) ", entries.len())
    } else {
        format!(" Git worktrees  /{q}  ({}) ", entries.len())
    };

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoWorktreesItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let mark = if w.is_current { "* " } else { "  " };
                let label = if w.is_bare {
                    "(bare)".to_string()
                } else {
                    w.branch.clone().unwrap_or_else(|| "(detached)".to_string())
                };
                let label_span = if w.is_current {
                    Span::styled(label, Style::new().fg(Color::Green).bold())
                } else {
                    Span::from(label)
                };
                let head = w.head.as_deref().unwrap_or("-------");
                let path = crate::app::home_relative(&w.path);
                let mut spans = vec![
                    Span::from(mark.to_string()).fg(Color::Green),
                    label_span,
                    Span::from(format!("  {head}  {path}")),
                ];
                if w.is_main {
                    spans.push(Span::from("  main").dim());
                }
                if w.is_bare {
                    spans.push(Span::from("  bare").dim());
                }
                if let Some(reason) = &w.locked {
                    let text = if reason.is_empty() {
                        "  locked".to_string()
                    } else {
                        format!("  locked: {reason}")
                    };
                    spans.push(Span::from(text).dim());
                }
                if w.prunable {
                    spans.push(Span::from("  prunable").dim());
                }
                let line = Line::from(spans);
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };

    let visible = area.height.saturating_sub(2) as usize;
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };
    let widget = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .scroll((offset, 0));
    frame.render_widget(widget, area);
}

/// Render the commit graph (`G`, SourceTree / Git Graph style).
/// Each row = colored lanes (derived from git's `--graph --color`) + refs label + subject + (short hash, author, date).
/// Only commit rows can hold the cursor; the selected row is reversed. Scrolls vertically to keep the selection visible.
pub fn render_graph(frame: &mut Frame, app: &App, area: Rect) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Color, Modifier};
    let lang = app.lang;
    let icons_on = app.cfg.ui.icons;
    let rows = app.git_graph_rows();
    let sel = app.git_graph_sel();
    let shown = app.list_visible(ListId::Graph);
    let is_shown = |i: usize| shown.as_ref().is_none_or(|v| v.binary_search(&i).is_ok());
    let q = app.list_filter_query(ListId::Graph);
    let q_tag = if q.is_empty() {
        String::new()
    } else {
        format!(" /{q}")
    };
    let n = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| r.commit.is_some() && is_shown(*i))
        .count();
    // While a base branch is pinned (Phase 2), also show base: <name> in the title. The "base:"
    // word is always spelled out (already self-descriptive in English); only the leading glyph is
    // gated by `ui.icons`, so icons=false never duplicates the wording ("⌖ base:" → "base:", not
    // "base: base:").
    let title = match app.git_graph_base_label() {
        // A pinned base is git's: jj asks the same question with a revset, so showing a base here
        // would name something that has no effect on what is drawn.
        Some(b) if crate::vcs::caps(&app.tab.root).write => {
            let icon = if icons_on {
                format!("{} ", icons::base_icon())
            } else {
                String::new()
            };
            format!(" {} graph{q_tag}  ({n})  {icon}base: {b} ", repo_name(app))
        }
        _ => format!(" {} graph{q_tag}  ({n}) ", repo_name(app)),
    };

    // Draw the border first, then split inner into the "commit column" and the "legend (bottom edge)".
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Legend (branch ⇄ lane color). Reserve 1-2 rows based on the chip width. The divider is dim (more subdued than the border).
    let legend = app.git_graph_legend();
    let hidden = app.git_graph_hidden_count();
    let est_w: usize = legend
        .iter()
        .map(|e| e.name.chars().count() + 5)
        .sum::<usize>()
        + if hidden > 0 { 12 } else { 0 };
    let chip_lines: u16 = if est_w <= inner.width.max(1) as usize {
        1
    } else {
        2
    };
    let show_legend = !legend.is_empty() && inner.height >= 3 + chip_lines;
    let (commit_area, legend_strip) = if show_legend {
        let parts =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1 + chip_lines)]).split(inner);
        (parts[0], Some(parts[1]))
    } else {
        (inner, None)
    };

    // --- Commit rows ---
    // How meta (short hash, author, date) is aligned. "inline"=right after the subject / anything else (default)=a right-aligned column.
    let align_right = app.cfg.ui.commit_meta_align != "inline";
    let cw = commit_area.width as usize;
    let lines: Vec<Line> = if rows.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoCommitsItem).to_string()).dim(),
        )]
    } else {
        rows.iter()
            .enumerate()
            .filter(|(i, _)| is_shown(*i))
            .map(|(i, r)| {
                // The colored graph part (lanes) becomes the starting point on the left.
                let mut left: Vec<Span> = r
                    .graph
                    .iter()
                    .map(|(t, st)| Span::styled(t.clone(), graph_style(*st)))
                    .collect();
                let line = if r.worktree {
                    // The uncommitted working-tree row: subject highlighted in yellow + date (meta).
                    left.push(Span::from(" "));
                    left.push(Span::styled(
                        r.subject.clone(),
                        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ));
                    let meta = r.date.clone();
                    if align_right && !meta.is_empty() {
                        Line::from(line_with_right_meta(left, meta, cw))
                    } else {
                        if !meta.is_empty() {
                            left.push(Span::from(format!("  {meta}")).dim());
                        }
                        Line::from(left)
                    }
                } else if r.commit.is_some() {
                    left.push(Span::from(" "));
                    if !r.refs.is_empty() {
                        // Branch/tag label (equivalent to SourceTree's chip).
                        left.push(Span::styled(
                            format!("({}) ", r.refs),
                            Style::new().fg(Color::Yellow),
                        ));
                    }
                    left.push(Span::from(r.subject.clone()));
                    // Meta info with empty fields dropped (short · author · date).
                    let meta = [r.short.clone(), r.author.clone(), r.date.clone()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if align_right && !meta.is_empty() {
                        Line::from(line_with_right_meta(left, meta, cw))
                    } else {
                        if !meta.is_empty() {
                            left.push(Span::from(format!("  {meta}")).dim());
                        }
                        Line::from(left)
                    }
                } else {
                    // A connector-only row with no commit (lanes only).
                    Line::from(left)
                };
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };
    let visible = commit_area.height as usize;
    let sel = position_among(&shown, sel);
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), commit_area);

    // --- Legend strip (divider + color chips) ---
    if let Some(strip) = legend_strip {
        let div = Rect {
            x: strip.x,
            y: strip.y,
            width: strip.width,
            height: 1,
        };
        let chips = Rect {
            x: strip.x,
            y: strip.y + 1,
            width: strip.width,
            height: chip_lines,
        };
        frame.render_widget(
            Paragraph::new(Line::from(
                Span::from("─".repeat(strip.width as usize)).fg(Color::DarkGray),
            )),
            div,
        );
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut spans: Vec<Span> = Vec::new();
        for (k, e) in legend.iter().enumerate() {
            let color = vcs_color(e.color);
            if k > 0 {
                spans.push(Span::from("  "));
            }
            // HEAD=⎇(icons=false: "br:")+bold / base=⌖(icons=false: "base:") / anything else=●.
            // The color is that lane's color. A space always separates the marker from the dot
            // (never glued directly onto it) — the marker glyph's fallback rendering can be wider
            // than 1 cell (see `icons::branch_marker`'s doc), and without the space it can spill
            // onto, and visually merge with, the dot.
            if e.is_head {
                spans.push(Span::styled(icons::branch_marker(icons_on), bold));
                spans.push(Span::from(" "));
                spans.push(Span::styled("●", bold.fg(color)));
                spans.push(Span::styled(format!(" {}", e.name), bold));
            } else if e.is_base {
                spans.push(Span::styled(
                    icons::base_marker(icons_on),
                    Style::new().fg(color),
                ));
                spans.push(Span::from(" "));
                spans.push(Span::styled("●", Style::new().fg(color)));
                spans.push(Span::from(format!(" {}", e.name)));
            } else {
                spans.push(Span::styled("●", Style::new().fg(color)));
                spans.push(Span::from(format!(" {}", e.name)));
            }
        }
        if hidden > 0 {
            spans.push(Span::from("  "));
            spans.push(Span::styled(
                format!(
                    "(+{hidden} {})",
                    tr(lang, crate::i18n::Msg::GraphLegendHidden)
                ),
                Style::new().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).wrap(ratatui::widgets::Wrap { trim: true }),
            chips,
        );
    }
}

/// The graph's branch visibility panel (`b`). Toggle show/hide via a checklist. Puts **HEAD first**.
/// Row: `[x]/[ ]` + current marker (⎇) + branch name. The selected row is reversed, hidden ones are dim, HEAD is green bold.
pub fn render_graph_picker(frame: &mut Frame, app: &App, area: Rect) {
    use ratatui::style::Color;
    let lang = app.lang;
    let icons_on = app.cfg.ui.icons;
    let items = app.git_graph_picker_items(); // (name, is_current, on), HEAD first
    let sel = app.git_graph_picker_sel();
    let shown = items.iter().filter(|(_, _, on)| *on).count();
    let title = format!(
        " {}  ({}/{}) ",
        tr(lang, crate::i18n::Msg::GraphPickerTitle),
        shown,
        items.len()
    );

    let lines: Vec<Line> = if items.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoBranchesItem).to_string()).dim(),
        )]
    } else {
        items
            .iter()
            .enumerate()
            .map(|(i, (name, is_cur, on))| {
                let check = if *on { "[x] " } else { "[ ] " };
                let check_span = if *on {
                    Span::styled(check, Style::new().fg(Color::Green))
                } else {
                    Span::from(check).dim()
                };
                // A 2-column-fixed marker: the branch-name column must start at the same x for
                // every row regardless of `is_cur`/`icons_on`, so both the "current" glyph/label
                // and its ASCII fallback are exactly 2 cells wide, matching the blank "  " used
                // for every other row (same convention as `render_branches`'s "* "/"  ").
                let marker = if *is_cur {
                    if icons_on {
                        format!("{} ", icons::branch_icon())
                    } else {
                        "* ".to_string()
                    }
                } else {
                    "  ".to_string()
                };
                let name_span = if *is_cur {
                    Span::styled(name.clone(), Style::new().fg(Color::Green).bold())
                } else if *on {
                    Span::from(name.clone())
                } else {
                    Span::from(name.clone()).dim()
                };
                let line = Line::from(vec![check_span, Span::from(marker), name_span]);
                if i == sel {
                    line.reversed()
                } else {
                    line
                }
            })
            .collect()
    };

    let visible = area.height.saturating_sub(2) as usize;
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };
    let widget = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .scroll((offset, 0));
    frame.render_widget(widget, area);
}

/// Render the commit detail (the DiffLine sequence from commit_diff) as a full-screen diff.
/// Uses the same Zed-style coloring as phase 3's GitDiff (shared `preview::gitdiff::diff_lines`).
/// The "── path ──" at the start of each file is inserted by git as a Context line.
/// What a revision is called, and which of its two IDs names it. git has one ID and calls it a
/// commit; jj keeps the change ID stable across rewrites and calls the thing a change.
fn revision_label(
    app: &App,
    meta: Option<&crate::git::CommitMeta>,
    fallback: Option<&str>,
) -> String {
    let (word, id) = match app.git_vcs {
        crate::vcs::VcsKind::Git => ("commit", meta.map(|m| m.short.clone())),
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => ("change", meta.map(|m| m.short.clone())),
    };
    match id.or_else(|| fallback.map(|f| f[..f.len().min(7)].to_string())) {
        Some(id) => format!(" {word} {id} "),
        None => format!(" {word} "),
    }
}

/// The meta info lines stacked at the top of the commit detail (the full commit message, preserving body line breaks).
/// Since log/graph can only show the one-liner subject, the detail view shows the full text.
fn commit_header_lines(
    meta: &crate::git::CommitMeta,
    width: usize,
    word: &str,
) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Modifier};
    let dim = Style::new().fg(Color::DarkGray);
    let mut out: Vec<Line<'static>> = Vec::new();
    // Line 1: commit <short>   <author> · <date>
    out.push(Line::from(vec![
        Span::styled(
            format!("{} {}", word, meta.short),
            Style::new().fg(Color::Yellow),
        ),
        Span::styled(format!("   {} · {}", meta.author, meta.date), dim),
    ]));
    out.push(Line::from(""));
    // The full message: the first line is the subject (bold), the rest is the body (kept as-is including blank lines = line breaks preserved).
    let mut msg = meta.message.lines();
    if let Some(subject) = msg.next() {
        out.push(Line::from(Span::styled(
            subject.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        )));
    }
    for body in msg {
        out.push(Line::from(body.to_string()));
    }
    out.push(Line::from(""));
    // A divider (dim), visually separating the message body from the diff.
    out.push(Line::from(Span::styled("─".repeat(width.max(1)), dim)));
    out
}

pub fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let split = app.diff_is_split(Block::bordered().inner(area).width);
    let mode_tag = if split { " ⇆" } else { "" };
    // Title: an explicit override (all changes from the git view) > the graph's selected row (worktree/commit) > the log selection.
    let base_title = if let Some(t) = app.git_detail_title() {
        format!(" {t} ")
    } else {
        match app.git_graph_selected_row() {
            Some(r) if r.worktree => {
                format!(" {} ", tr(app.lang, crate::i18n::Msg::UncommittedChanges))
            }
            Some(r) => revision_label(app, app.git_detail_meta(), r.commit.as_deref()),
            None => revision_label(
                app,
                app.git_detail_meta(),
                app.git_log_selected_id().as_deref(),
            ),
        }
    };
    let title = format!("{}{} ", base_title.trim_end(), mode_tag);
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.set_git_detail_viewport(inner.height);

    // Stack the commit meta (full message) at the top. Build it owned first to release app's borrow.
    let iw = inner.width as usize;
    let header_lines: Vec<Line<'static>> = app
        .git_detail_meta()
        .map(|m| {
            let word = match app.git_vcs {
                crate::vcs::VcsKind::Git => "commit",
                #[cfg(feature = "git")]
                crate::vcs::VcsKind::Jj => "change",
            };
            commit_header_lines(m, iw, word)
        })
        .unwrap_or_default();

    let diff = app.git_detail_lines();
    if diff.is_empty() {
        // No diff (an empty commit etc.). If there's meta, show at least the message; otherwise the usual centered display.
        if header_lines.is_empty() {
            frame.render_widget(block, area);
            let msg = tr(app.lang, crate::i18n::Msg::GitNoChanges);
            let y = inner.y + inner.height / 2;
            let line_area = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), line_area);
            app.set_git_detail_total(0);
            return;
        }
        let mut lines = header_lines;
        lines.push(Line::from(Span::styled(
            tr(app.lang, crate::i18n::Msg::GitNoChanges),
            Style::new().fg(ratatui::style::Color::DarkGray),
        )));
        app.clamp_git_detail_hscroll(0);
        app.set_git_detail_total(lines.len());
        let scroll = clamp_vscroll(app.git_detail_scroll(), lines.len(), inner.height as usize);
        let para = Paragraph::new(Text::from(lines))
            .block(block)
            .scroll((scroll, 0));
        frame.render_widget(para, area);
        return;
    }

    // A commit diff spans multiple files (already separated by headers). The body is colored per file's extension.
    let theme = app.cfg.ui.theme.code_theme.clone();
    let cur_h = app.git_detail_hscroll(); // read this before diff (= app's borrow)
                                          // Horizontal scroll: stacked uses Paragraph's horizontal offset; side-by-side shifts only each column's body internally (a fixed gutter).
    let (body_lines, para_hscroll, clamped_h) = if split {
        let max_h = crate::preview::gitdiff::side_by_side_max_hscroll(diff, iw) as u16;
        let h = cur_h.min(max_h);
        let lines =
            crate::preview::gitdiff::diff_lines_side_by_side(diff, "", &theme, iw, h as usize);
        (lines, 0u16, h)
    } else {
        let lines = crate::preview::gitdiff::diff_lines(diff, "", &theme, iw);
        // The horizontal-scroll cap accounts for both the header (its body can be long) and the diff.
        let max_h = header_lines
            .iter()
            .chain(lines.iter())
            .map(|l| l.width())
            .max()
            .unwrap_or(0)
            .saturating_sub(iw) as u16;
        let h = cur_h.min(max_h);
        (lines, h, h)
    };
    // diff's borrow has ended, so apply the clamped value (kept for the next render too).
    app.clamp_git_detail_hscroll(clamped_h);

    // Stack the header (full message) at the top, then continue with the diff.
    let mut lines = header_lines;
    lines.extend(body_lines);

    // Clamp the vertical scroll amount so it never overshoots the tail (no wrapping = the horizontal is truncated).
    // The scroll cap is determined by the total line count including the header, so the render side reports it to App.
    app.set_git_detail_total(lines.len());
    let scroll = clamp_vscroll(app.git_detail_scroll(), lines.len(), inner.height as usize);

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, para_hscroll));
    frame.render_widget(para, area);
}

/// Clamp the vertical scroll amount to the bottom (= total line count - viewport height).
/// The max is computed in **usize**: casting `total_lines` to u16 too early would
/// wrap the max for diffs over 65535 lines, making the bottom unreachable.
/// Only at the final step, where ratatui's `scroll` requires u16, do we saturate down to u16.
fn clamp_vscroll(scroll: u16, total_lines: usize, viewport_h: usize) -> u16 {
    let max_scroll = total_lines.saturating_sub(viewport_h);
    (scroll as usize).min(max_scroll).min(u16::MAX as usize) as u16
}

/// The `?` help for the Git overlay (`o`/`L`/`G`/`b`/detail): the sections of the active screen,
/// generated by `ops::help`.
#[allow(dead_code)]
pub fn help_sections(app: &App) -> Vec<crate::ui::help::HelpSection> {
    crate::ops::help::sections(app)
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use super::*;

    // Regression: even a huge diff over 65535 lines can reach the tail without the cap wrapping in u16.
    #[test]
    fn clamp_vscroll_reaches_tail_for_huge_diff() {
        // Total lines 70000, viewport 50 → the true cap is 69950.
        // Casting to u16 too early would turn 69950 % 65536 into 4414, never reaching the tail.
        let total = 70_000usize;
        let viewport = 50usize;
        // The scroll state's cap is u16 (65535). Even sending the max still advances near the tail.
        assert_eq!(clamp_vscroll(u16::MAX, total, viewport), u16::MAX);
        // A small offset passes through unchanged.
        assert_eq!(clamp_vscroll(100, total, viewport), 100);
        // If there are fewer lines than the tail, it's clamped at the cap.
        assert_eq!(clamp_vscroll(500, 120, viewport), 70);
        // Doesn't panic even with 0 lines / an empty viewport.
        assert_eq!(clamp_vscroll(10, 0, 0), 0);
    }

    // --- ⎇/⌖ gated by ui.icons, never glued directly to the next glyph ----------------------
    // U+2387 (⎇) / U+2316 (⌖) are absent from Menlo/SF Mono/Monaco/Courier/Andale Mono/Courier
    // New/HackGen Console NF (fontTools cmap dump), so macOS always falls back to Apple Symbols
    // — whose glyph advance is ~1.7x a Menlo cell. Before this fix, both glyphs were emitted
    // unconditionally (ignoring `ui.icons`) and, in the graph legend, glued directly onto the
    // following `●` with no space cell in between, so an oversized fallback glyph could spill
    // onto — and visually merge with — the lane dot.

    // -----------------------------------------------------------------------------------------
    // jj display-layer coverage (2026-08-19 audit). See the matching section in `ui::status`'s
    // tests for the full rationale; the short version: the data layer (`crates/konoma-vcs-core/src/vcs/jj.rs`) is well
    // tested, but nothing checked that jj's own `?`-help wording and `jj bookmarks` render
    // actually reach the screen instead of git's. Every test below is paired with git.
    // -----------------------------------------------------------------------------------------
}
