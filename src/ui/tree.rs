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
            break; // 末尾の `…`(幅1)分を残す
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
    // name の各文字を小文字化しつつ、その文字が name 上で占めるバイト範囲 [start,end) を保持する。
    // to_lowercase は 1 文字を複数文字に展開し得るので、小文字単位ごとに同じ name バイト範囲を紐付ける。
    let mut units: Vec<(char, usize, usize)> = Vec::new();
    for (b, ch) in name.char_indices() {
        let end = b + ch.len_utf8();
        for lc in ch.to_lowercase() {
            units.push((lc, b, end));
        }
    }
    // クエリに一致した name バイト範囲を昇順・非重複で集める(範囲端は常に name の文字境界)。
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
                // 直前の範囲と接する/重なる場合は結合(同一文字内の重複強調を避ける)。
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
    // 現在の並び (例: "sort: mod ↑")。
    let mut spans = vec![Span::from(format!("  {}", app.sort_label())).dim()];
    // 選択/ビジュアル中は件数 (例: "sel: 3")。
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
    // クリップボードに積まれていれば表示 (例: "[copy 3]")。ペースト可能の合図。
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
        // ファイル管理(Space リーダー)は実際のキーマップ(設定反映済み)から組み立てる。
        // 削除は y=ゴミ箱 / !=完全削除、作成は末尾 / でフォルダ。
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
    // 複数タブなら q=タブを閉じる(＋Q=終了)、最後の1つなら q=終了。
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
    // root が変わっていれば git status を取り直す (FR-7)。同一 root では再計算しない。
    app.refresh_git_if_needed();

    let icons_on = app.cfg.ui.icons;
    // 変更が1つでもあれば、整列のため全行の先頭に状態ガター(2桁)を出す。
    let show_gutter = app.git_has_changes();
    // 複数選択中/ビジュアル中は、各行の最左に選択マーカー列(2桁)を出す(空なら出さない)。
    let show_sel = app.show_selection_gutter();
    // 絞り込み中/変更のみ表示中はフラットな結果一覧なので、各行に root からの相対パスを出して場所が分かるようにする。
    let filtering = app.filter_query().is_some() || app.changed_filter();
    let query = app.filter_query().unwrap_or("").to_string();
    let root_for_rel = app.root.clone();
    // 詳細リスト列 (設定 [ui] details)。有効な列のみ採用し、右端に固定幅で並べる。
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
        .map(|id| 2 + fileops::detail_column_width(id).unwrap()) // "  " 区切り + 列幅
        .sum();
    let inner_w = area.width.saturating_sub(2) as usize; // 左右ボーダー
    let name_region_w = inner_w.saturating_sub(details_w); // 名前＋パディングの領域

    // 可視範囲だけを整形する(C3)。数千エントリでも整形/`quick_meta` の stat syscall を
    // 画面に映る行(≒ビューポート高)に限定し、スクロールごとの全件再整形を避ける。
    // 選択行が常に見えるようスクロール量を先に決め、`[offset..end]` だけを Line 化する。
    let visible = area.height.saturating_sub(2) as usize; // 上下ボーダー分を除く
    app.tree_viewport = visible as u16; // ページ送りの1ページ量に使う
    let offset = if visible > 0 && app.selected >= visible {
        app.selected - visible + 1
    } else {
        0
    };
    let end = offset.saturating_add(visible).min(app.entries.len());
    let lines: Vec<Line> = app.entries[offset..end]
        .iter()
        .enumerate()
        .map(|(vi, e)| {
            let i = offset + vi; // entries 内の元の添字(選択/ビジュアル判定に使う)
            let indent = "  ".repeat(e.depth);
            let fname = || {
                e.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string()
            };
            // 絞り込み中は root からの相対パス、通常はファイル名(所有 String にして Line に載せる)。
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

            // 行頭(インデント前)= 複数選択マーカー → git 状態ガター。色＝意味。
            let mut spans: Vec<Span> = Vec::new();
            if show_sel {
                // 確定済み選択 ∪ ビジュアルの進行中範囲をマーク表示。
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
            // gitignore 除外(自身 or 祖先が ignored)は Zed 風に少し暗く(DIM)する。
            // 変更ステータス(色=意味)があるエントリは従来どおり着色を優先(ignored 扱いしない)。
            let ignored = status.is_none() && app.is_ignored(&e.path);
            let dim = Style::new().add_modifier(Modifier::DIM);
            spans.push(Span::styled(
                prefix,
                if ignored { dim } else { Style::default() },
            ));
            // 名前は git 状態色 > ignored(暗く) > 既定色(テーマ追従)。
            let name_style = if let Some(st) = status {
                Style::new().fg(st.color())
            } else if ignored {
                dim
            } else {
                Style::default()
            };
            if show_details {
                // 名前を領域幅に詰め、残りを空白で埋めてから固定幅の列を右端に並べる(縦に揃う)。
                let left_w = (show_sel as usize) * 2 + (show_gutter as usize) * 2 + prefix_w;
                let budget = name_region_w.saturating_sub(left_w);
                let (tname, nw) = truncate_width(&name, budget);
                spans.push(Span::styled(tname, name_style));
                let pad = name_region_w.saturating_sub(left_w + nw);
                if pad > 0 {
                    spans.push(Span::from(" ".repeat(pad)));
                }
                let meta = fileops::quick_meta(&e.path).unwrap_or(fileops::RowMeta {
                    is_dir: e.is_dir,
                    is_symlink: false,
                    size: 0,
                    mtime: None,
                    mode: 0,
                });
                for id in &detail_cols {
                    let w = fileops::detail_column_width(id).unwrap();
                    let cell = fileops::detail_cell(id, &e.path, &meta).unwrap_or_default();
                    let (cell_t, _) = truncate_width(&cell, w);
                    spans.push(Span::from(format!("  {cell_t:>w$}")).dim());
                }
            } else if filtering && !query.is_empty() {
                // 絞り込み中はマッチ箇所(クエリ部分)を強調する。
                spans.extend(highlight_match(&name, &query, name_style));
            } else {
                spans.push(Span::styled(name, name_style));
            }

            let line = Line::from(spans);
            if i == app.selected {
                line.reversed() // 選択行は反転 (色付き span も含む)
            } else {
                line
            }
        })
        .collect();

    let root = app.root.clone();
    // タイトル枠にパス＋(git リポジトリなら) ブランチ名を併記する。絞り込み中はクエリ件数も、
    // 変更のみ表示中は変更件数を出す。
    let title = match (app.filter_query(), app.git_branch()) {
        (Some(q), _) => format!(
            " {}  /{}  ({}) ",
            app.format_path(&root),
            q,
            app.entries.len()
        ),
        (None, _) if app.changed_filter() => format!(
            " {}  ± {} ({}) ",
            app.format_path(&root),
            tr(app.lang, Msg::StChangedOnly),
            app.entries.len()
        ),
        (None, Some(branch)) => format!(" {}  ⎇ {} ", app.format_path(&root), branch),
        (None, None) => format!(" {} ", app.format_path(&root)),
    };

    // 既に `[offset..end]` だけを Line 化済みなので、Paragraph 側の縦スクロールは 0
    // (offset 行ぶんは描かずに飛ばすのではなく、最初から作っていない)。
    let widget = Paragraph::new(lines).block(Block::bordered().title(title));
    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_match_marks_query_in_yellow() {
        let spans = highlight_match("src/main.rs", "rs", Style::default());
        // 連結すると元の文字列に戻る(欠落/重複なし)。
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "src/main.rs");
        // マッチ部 "rs" の span が黄色＋太字で強調される。
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
        // 非 ASCII 名でも強調が出る(silent degradation しない)。
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
        // 小文字化で文字境界がズレ得る組合せでも panic せず、連結で元に戻る。
        // 'İ'(U+0130) は to_lowercase で 'i' + 結合ドットの 2 文字に展開される。
        let spans = highlight_match("İẞ", "ß", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "İẞ");
        // 'ẞ'(U+1E9E) は小文字 'ß' に一致し強調される。
        assert!(
            spans
                .iter()
                .any(|s| s.content.as_ref() == "ẞ" && s.style.fg == Some(Color::Yellow)),
            "強調が出ていない: {spans:?}"
        );
    }

    #[test]
    fn help_sections_lists_tree_keys_and_leaders() {
        let dir = std::env::temp_dir().join("konoma_help_sections_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let app = App::new(dir.clone(), crate::config::Config::default()).unwrap();
        let secs = help_sections(&app);
        assert!(
            secs.len() >= 3,
            "ツリー/ファイル管理/選択など複数セクション"
        );
        // 先頭セクションはツリー操作。代表的なキー行を含む。
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
        // どのセクションも空ではない(タイトルだけのセクションは作らない方針の確認)。
        assert!(
            secs.iter().all(|s| !s.title.is_empty()),
            "空タイトルのセクションがある"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
