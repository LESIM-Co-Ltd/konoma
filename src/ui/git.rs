// Git ビュー(変更ハブ)の描画。`o` で開く全画面の変更ファイル一覧。
// 各行: ステージ済み指標(●/空) + 状態マーカー(色=意味) + repo からの相対パス。
// 選択行は反転。選択が常に見えるようスクロールする。
//
// 一覧データは App.git_view_entries() が持つ(open/refresh/書込後に作り直し)。
// diff 表示・log は別マイルストーン(phase 3/5)。ここは一覧と staged 表示のみ。

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

/// Render the Git view's change list full-screen, in the content area instead of tree/preview.
pub fn render_changes(frame: &mut Frame, app: &App, area: Rect) {
    let lang = app.lang;
    let entries = app.git_view_entries();
    let sel = app.git_view_sel();

    // タイトル: " Git ⎇ <branch>  (N) "。ブランチが無ければ記号のみ。
    let title = match app.git_branch() {
        Some(b) => format!(" Git ⎇ {b}  ({}) ", entries.len()),
        None => format!(" Git  ({}) ", entries.len()),
    };

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoChangesItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let st = e.status;
                // staged 指標: ●=ステージ済 / 空白=未ステージ。
                let staged = if e.staged { "● " } else { "  " };
                // 状態マーカー(色=意味)。
                let marker = Span::styled(format!("{} ", st.marker()), Style::new().fg(st.color()));
                // repo からの相対パス(format_path で表示スタイルに従う)。
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

    // 選択行が常に見えるよう縦スクロール量を決める(tree と同じ素朴な追従)。
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
    let budget = max - 1; // 末尾 `…`(幅1)分を残す
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
    // メタ＋最低 1 空白すら置けないほど狭い → メタは諦め、左側だけ出す。
    if meta_w + 2 > width {
        return truncate_spans(left, width);
    }
    let left_budget = width - meta_w - 1; // 最低 1 空白を確保
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
    let entries = app.git_log_entries();
    let sel = app.git_log_sel();

    let title = match app.git_branch() {
        Some(b) => format!(" Git log ⎇ {b}  ({}) ", entries.len()),
        None => format!(" Git log  ({}) ", entries.len()),
    };

    // メタ(author・日付)の寄せ方。"inline"=subject 直後 / それ以外(既定)=右端寄せ列。
    let align_right = app.cfg.ui.commit_meta_align != "inline";
    let inner_w = area.width.saturating_sub(2) as usize; // 枠 2 桁を除いた本文幅

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoCommitsItem).to_string()).dim(),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, c)| {
                // time_epoch は i64。負(1970 以前)は 0 に丸めて u64 の整形器へ渡す。
                let date = crate::fileops::format_epoch_short(c.time_epoch.max(0) as u64);
                let line = if align_right {
                    // 短縮ハッシュ(左の固定アンカー) + summary を左、author・日付を右端列に。
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
    let entries = app.git_branch_view(); // 絞り込み後の表示リスト
    let sel = app.git_branch_sel();
    // 絞り込み中/クエリありはタイトルに "/<query>" を出す。
    let q = app.git_branch_query();
    let title = if q.is_empty() {
        format!(" Git branches  ({}) ", entries.len())
    } else {
        format!(" Git branches  /{q}  ({}) ", entries.len())
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

/// Render the commit graph (`G`, SourceTree / Git Graph style).
/// Each row = colored lanes (derived from git's `--graph --color`) + refs label + subject + (short hash, author, date).
/// Only commit rows can hold the cursor; the selected row is reversed. Scrolls vertically to keep the selection visible.
pub fn render_graph(frame: &mut Frame, app: &App, area: Rect) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Color, Modifier};
    let lang = app.lang;
    let rows = app.git_graph_rows();
    let sel = app.git_graph_sel();
    let n = rows.iter().filter(|r| r.commit.is_some()).count();
    // 基準ブランチ固定中(Phase 2)はタイトルに base: <名> を併記する。
    let title = match app.git_graph_base_label() {
        Some(b) => format!(" Git graph  ({n})  ⌖ base: {b} "),
        None => format!(" Git graph  ({n}) "),
    };

    // 枠を先に描き、inner を「コミット列」と「凡例(下端)」に分ける。
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 凡例(ブランチ⇄レーン色)。チップ幅から 1〜2 行を確保。区切り線は dim(枠より控えめ)。
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

    // --- コミット行 ---
    // メタ(short・author・日付)の寄せ方。"inline"=subject 直後 / それ以外(既定)=右端寄せ列。
    let align_right = app.cfg.ui.commit_meta_align != "inline";
    let cw = commit_area.width as usize;
    let lines: Vec<Line> = if rows.is_empty() {
        vec![Line::from(
            Span::from(tr(lang, crate::i18n::Msg::GitNoCommitsItem).to_string()).dim(),
        )]
    } else {
        rows.iter()
            .enumerate()
            .map(|(i, r)| {
                // 色付きグラフ部(レーン)を左側の起点にする。
                let mut left: Vec<Span> = r
                    .graph
                    .iter()
                    .map(|(t, st)| Span::styled(t.clone(), *st))
                    .collect();
                let line = if r.worktree {
                    // 未コミットの作業ツリー行: 黄色強調の subject + 日付(メタ)。
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
                        // ブランチ/タグのラベル(SourceTree のチップ相当)。
                        left.push(Span::styled(
                            format!("({}) ", r.refs),
                            Style::new().fg(Color::Yellow),
                        ));
                    }
                    left.push(Span::from(r.subject.clone()));
                    // 空フィールドを除いたメタ情報 (short · author · date)。
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
                    // コミットを持たない接続専用行(レーンのみ)。
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
    let offset = if visible > 0 && sel >= visible {
        (sel - visible + 1) as u16
    } else {
        0
    };
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), commit_area);

    // --- 凡例 strip(区切り線 + 色チップ) ---
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
            if k > 0 {
                spans.push(Span::from("  "));
            }
            // HEAD=⎇＋太字 / 基準=⌖ / それ以外=●。色はそのレーン色。
            if e.is_head {
                spans.push(Span::styled("⎇", bold));
                spans.push(Span::styled("●", bold.fg(e.color)));
                spans.push(Span::styled(format!(" {}", e.name), bold));
            } else if e.is_base {
                spans.push(Span::styled("⌖●", Style::new().fg(e.color)));
                spans.push(Span::from(format!(" {}", e.name)));
            } else {
                spans.push(Span::styled("●", Style::new().fg(e.color)));
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
    let items = app.git_graph_picker_items(); // (name, is_current, on) HEAD 先頭
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
                let marker = if *is_cur { "⎇ " } else { "  " };
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
/// The meta info lines stacked at the top of the commit detail (the full commit message, preserving body line breaks).
/// Since log/graph can only show the one-liner subject, the detail view shows the full text.
fn commit_header_lines(meta: &crate::git::CommitMeta, width: usize) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Modifier};
    let dim = Style::new().fg(Color::DarkGray);
    let mut out: Vec<Line<'static>> = Vec::new();
    // 1行目: commit <short>   <author> · <date>
    out.push(Line::from(vec![
        Span::styled(
            format!("commit {}", meta.short),
            Style::new().fg(Color::Yellow),
        ),
        Span::styled(format!("   {} · {}", meta.author, meta.date), dim),
    ]));
    out.push(Line::from(""));
    // メッセージ全文: 先頭行=件名(太字)、以降=本文(空行も含めそのまま=改行を保持)。
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
    // 区切り線(dim)。メッセージ本文と diff を視覚的に分ける。
    out.push(Line::from(Span::styled("─".repeat(width.max(1)), dim)));
    out
}

pub fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let split = app.diff_is_split(Block::bordered().inner(area).width);
    let mode_tag = if split { " ⇆" } else { "" };
    // タイトル: 明示上書き(git ビューからの全変更)> グラフ選択行(worktree/commit) > log 選択。
    let base_title = if let Some(t) = app.git_detail_title() {
        format!(" {t} ")
    } else {
        match app.git_graph_selected_row() {
            Some(r) if r.worktree => {
                format!(" {} ", tr(app.lang, crate::i18n::Msg::UncommittedChanges))
            }
            Some(r) => match &r.commit {
                Some(id) => format!(" commit {} ", &id[..id.len().min(7)]),
                None => " commit ".to_string(),
            },
            None => match app.git_log_selected_id() {
                Some(id) => format!(" commit {} ", &id[..id.len().min(7)]),
                None => " commit ".to_string(),
            },
        }
    };
    let title = format!("{}{} ", base_title.trim_end(), mode_tag);
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.set_git_detail_viewport(inner.height);

    // コミットメタ(完全メッセージ)を先頭に積む。先に owned で作って app の借用を解放しておく。
    let iw = inner.width as usize;
    let header_lines: Vec<Line<'static>> = app
        .git_detail_meta()
        .map(|m| commit_header_lines(m, iw))
        .unwrap_or_default();

    let diff = app.git_detail_lines();
    if diff.is_empty() {
        // 差分なし(空コミット等)。メタがあればメッセージだけは出す。なければ従来通り中央表示。
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

    // コミット差分は複数ファイルにまたがる(ヘッダで区切り済み)。本文はファイル別 ext で着色する。
    let theme = app.cfg.ui.theme.code_theme.clone();
    let cur_h = app.git_detail_hscroll(); // diff(=app の借用)より先に読んでおく
                                          // 横スクロール: 縦並びは Paragraph の横 offset、横並びは各列の本文だけを内部でずらす(固定ガター)。
    let (body_lines, para_hscroll, clamped_h) = if split {
        let max_h = crate::preview::gitdiff::side_by_side_max_hscroll(diff, iw) as u16;
        let h = cur_h.min(max_h);
        let lines =
            crate::preview::gitdiff::diff_lines_side_by_side(diff, "", &theme, iw, h as usize);
        (lines, 0u16, h)
    } else {
        let lines = crate::preview::gitdiff::diff_lines(diff, "", &theme, iw);
        // 横スクロール上限はヘッダ(長文本文がありうる)と diff の両方を考慮。
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
    // diff の借用が終わったのでクランプ後の値を反映(次回描画でも保持)。
    app.clamp_git_detail_hscroll(clamped_h);

    // ヘッダ(完全メッセージ)を先頭に積んで diff を続ける。
    let mut lines = header_lines;
    lines.extend(body_lines);

    // 末尾を超えないよう縦スクロール量をクランプ(折返ししない=横は切り捨て)。
    // スクロール上限はヘッダ込みの総行数で決まるので描画側から App へ伝える。
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

/// The `?` help for the Git overlay (`o`/`L`/`G`/`b`/detail). Returns the key section for the **currently active sub-view**.
/// (help_lines uses this instead of tree/preview when in Git mode. The common Tabs/Copy/Global sections are appended afterward.)
pub fn help_sections(app: &App) -> Vec<crate::ui::help::HelpSection> {
    use crate::ui::help::HelpSection;
    let lang = app.lang;
    let l = |m| tr(lang, m);
    let sec = if app.is_git_detail() {
        HelpSection::new(l(crate::i18n::Msg::GitCommitWorktreeDetail))
            .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
            .row("h / l  0 / $", l(crate::i18n::Msg::GitHScrollEnds))
            .row("Ctrl-d / Ctrl-u", l(crate::i18n::Msg::Scroll10Lines))
            .row("s", l(crate::i18n::Msg::GitLayout))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("q / Esc", l(crate::i18n::Msg::GitBack))
    } else if app.is_git_log() {
        HelpSection::new(l(crate::i18n::Msg::GitLogLabel))
            .row("j / k", l(crate::i18n::Msg::GitMove))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("Enter / l", l(crate::i18n::Msg::GitCommitDetail))
            .row("q / Esc", l(crate::i18n::Msg::BackToChanges))
    } else if app.is_git_graph() {
        HelpSection::new(l(crate::i18n::Msg::GitGraphLabel))
            .row("j / k", l(crate::i18n::Msg::GitMoveCommit))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("Enter / l", l(crate::i18n::Msg::GitDetail))
            .row("q / Esc", l(crate::i18n::Msg::BackToChanges))
    } else if app.is_git_branches() {
        HelpSection::new(l(crate::i18n::Msg::GitBranchesLabel))
            .row("j / k", l(crate::i18n::Msg::GitMove))
            .row("Enter / l", l(crate::i18n::Msg::GitCheckout))
            .row("n", l(crate::i18n::Msg::GitNewBranch))
            .row("d", l(crate::i18n::Msg::GitDelete))
            .row("/", l(crate::i18n::Msg::GitFilterByName))
            .row("q / Esc", l(crate::i18n::Msg::BackToChanges))
    } else {
        // 変更ハブ(o)
        HelpSection::new(l(crate::i18n::Msg::GitChangesLabel))
            .row("j / k", l(crate::i18n::Msg::GitMove))
            .row("s / S", l(crate::i18n::Msg::StageHint))
            .row("u / U", l(crate::i18n::Msg::UnstageHint))
            .row("x", l(crate::i18n::Msg::GitDiscardFile))
            .row("c", l(crate::i18n::Msg::GitCommit))
            .row("Enter", l(crate::i18n::Msg::GitFileDiff))
            .row("d", l(crate::i18n::Msg::GitDiffAll))
            .row("l / g", l(crate::i18n::Msg::GitLogGraph))
            .row("b", l(crate::i18n::Msg::GitBranches))
            .row("!", l(crate::i18n::Msg::GitExternalTool))
            .row("q / Esc", l(crate::i18n::Msg::GitCloseView))
    };
    vec![sec]
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use super::*;
    use crate::config::Config;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn init_repo(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        cfg.set_str("commit.gpgsign", "false").ok();
    }

    #[test]
    fn git_view_renders_marker_and_filename() {
        let dir = std::env::temp_dir().join("konoma_git_view_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("foo.txt"), b"hi\n").unwrap(); // untracked → 'U'
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_view();
        assert!(app.is_git_view(), "Git ビューが開かない");

        let mut term = Terminal::new(TestBackend::new(50, 8)).unwrap();
        term.draw(|f| render_changes(f, &app, f.area())).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("Git"), "タイトルが無い: {s}");
        assert!(s.contains('U'), "状態マーカーが無い: {s}");
        assert!(s.contains("foo.txt"), "ファイル名が無い: {s}");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn sh(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    }

    #[test]
    fn git_log_render_shows_commit_summary() {
        let dir = std::env::temp_dir().join("konoma_git_log_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"hi\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-m", "hello world commit"]);

        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_log();
        assert!(app.is_git_log(), "git log が開かない");

        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| render_log(f, &app, f.area())).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("log"), "ログタイトルが無い: {s}");
        assert!(
            s.contains("hello world commit"),
            "コミット summary が無い: {s}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn right_meta_aligns_pads_and_degrades() {
        use unicode_width::UnicodeWidthStr;
        let text =
            |spans: &[Span]| -> String { spans.iter().map(|s| s.content.to_string()).collect() };

        // ① 余裕あり: 左 + 空白詰め + メタ。総幅が width にぴったり(=右端揃え)。
        let out = line_with_right_meta(vec![Span::from("abc".to_string())], "date".to_string(), 20);
        let t = text(&out);
        assert!(t.starts_with("abc"), "左が先頭でない: {t}");
        assert!(t.ends_with("date"), "メタが右端でない: {t}");
        assert_eq!(
            UnicodeWidthStr::width(t.as_str()),
            20,
            "右端まで詰まっていない: {t}"
        );

        // ② 溢れ: 左(subject)を末尾省略しメタは残す。総幅は width 以内。
        let long = vec![Span::from("abcdefghijklmnopqrstuvwxyz".to_string())];
        let out = line_with_right_meta(long, "date".to_string(), 12);
        let t = text(&out);
        assert!(t.contains('…'), "省略記号が無い: {t}");
        assert!(t.ends_with("date"), "メタが残っていない: {t}");
        assert!(UnicodeWidthStr::width(t.as_str()) <= 12, "width 超過: {t}");

        // ③ 狭すぎ(meta幅+2 > width): メタを諦め、左のみ(末尾省略)。
        let out = line_with_right_meta(
            vec![Span::from("abcdef".to_string())],
            "longmeta".to_string(),
            5,
        );
        let t = text(&out);
        assert!(!t.contains("longmeta"), "狭いのにメタが残っている: {t}");
        assert!(UnicodeWidthStr::width(t.as_str()) <= 5, "width 超過: {t}");
    }

    #[test]
    fn git_branches_render_shows_current_marker() {
        let dir = std::env::temp_dir().join("konoma_git_branches_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"hi\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-m", "init"]);
        sh(&dir, &["branch", "feature"]);

        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_branches();
        assert!(app.is_git_branches(), "ブランチ一覧が開かない");

        let mut term = Terminal::new(TestBackend::new(50, 8)).unwrap();
        term.draw(|f| render_branches(f, &app, f.area())).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("branches"), "タイトルが無い: {s}");
        assert!(s.contains("feature"), "feature ブランチが無い: {s}");
        assert!(s.contains('*'), "現在ブランチの * マーカーが無い: {s}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_detail_render_shows_diff() {
        let dir = std::env::temp_dir().join("konoma_git_detail_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"alpha\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-m", "init"]);

        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_log();
        app.open_git_commit_detail();
        assert!(app.is_git_detail(), "コミット詳細が開かない");

        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| render_detail(f, &mut app, f.area())).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // 追加されたファイル内容("alpha")が詳細 diff に出る。
        assert!(s.contains("alpha"), "diff 本文が出ていない: {s}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // 回帰: 65535 行超の巨大 diff でも上限が u16 でラップせず末尾に到達できる。
    #[test]
    fn clamp_vscroll_reaches_tail_for_huge_diff() {
        // 総行数 70000, ビューポート 50 → 真の上限は 69950。
        // u16 早すぎキャストだと 69950 % 65536 = 4414 に化けて末尾に届かない。
        let total = 70_000usize;
        let viewport = 50usize;
        // スクロール状態の上限は u16(65535)。最大まで送っても末尾近くへ進める。
        assert_eq!(clamp_vscroll(u16::MAX, total, viewport), u16::MAX);
        // 小さな offset はそのまま通る。
        assert_eq!(clamp_vscroll(100, total, viewport), 100);
        // 末尾より行数が少なければ上限でクランプされる。
        assert_eq!(clamp_vscroll(500, 120, viewport), 70);
        // 0 行・空ビューポートでもパニックしない。
        assert_eq!(clamp_vscroll(10, 0, 0), 0);
    }

    #[test]
    fn commit_header_shows_full_multiline_message() {
        let meta = crate::git::CommitMeta {
            id: "abc1234567890abcdef1234567890abcdef123456".into(),
            short: "abc1234".into(),
            author: "Alice".into(),
            date: "2026-06-29".into(),
            message: "subject line\n\nbody paragraph one\nbody paragraph two".into(),
        };
        let lines = commit_header_lines(&meta, 40);
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        // 1行目に short/author/date。
        assert!(text[0].contains("abc1234"));
        assert!(text[0].contains("Alice"));
        assert!(text[0].contains("2026-06-29"));
        // 件名と本文(改行を保持=各行が独立)が全て含まれる。
        assert!(text.iter().any(|t| t == "subject line"));
        assert!(text.iter().any(|t| t == "body paragraph one"));
        assert!(text.iter().any(|t| t == "body paragraph two"));
        // 件名と本文の間の空行が保持されている。
        assert!(text.iter().any(|t| t.is_empty()));
        // 最終行は区切り線(dim)。
        assert!(text.last().unwrap().starts_with('─'));
        // 件名行は太字。
        let subj = lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    == "subject line"
            })
            .unwrap();
        assert!(subj.spans.iter().any(|s| s
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)));
    }

    #[test]
    fn graph_picker_renders_panel_title_and_branches() {
        let dir = std::env::temp_dir().join("konoma_graph_picker_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "init"]);
        sh(&dir, &["branch", "-M", "trunk"]);
        sh(&dir, &["branch", "feature-x"]);

        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_graph();
        app.git_graph_open_picker();
        assert!(app.is_git_graph_picker(), "パネルが開く");

        let mut term = Terminal::new(TestBackend::new(60, 16)).unwrap();
        term.draw(|f| render_graph_picker(f, &app, f.area()))
            .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // パネルタイトル(Graph branches)とブランチ名が並ぶ。
        assert!(
            s.contains(tr(app.lang, crate::i18n::Msg::GraphPickerTitle)),
            "パネルタイトルが無い: {s}"
        );
        assert!(s.contains("trunk"), "HEAD ブランチが無い: {s}");
        assert!(s.contains("feature-x"), "もう一方のブランチが無い: {s}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
