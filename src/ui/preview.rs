// プレビュー描画 (全画面)。
// テキスト系 (Markdown / Code / Text フォールバック) は実本文を読み込んで全画面スクロール表示する。
// 画像(M2)・外部コマンド委譲(M2+)はまだ種別要約のまま。Markdown のリッチ描画 (装飾/Mermaid) は M3。
//
// スクロール:
//   - 縦: 末尾を超えないよう描画時にクランプ (内容行数と画面高さがここで判るため)。
//   - 横: ui.wrap=false のとき長い行を見るために使う。折返し時は横スクロール無効 (0 にクランプ)。

use std::path::Path;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::{FilterType, Image, Resize, StatefulImage};

use crate::app::App;
use crate::i18n::tr;
use crate::preview::{text, PreviewKind};
use crate::ui::status::{hint, page_hint};

/// Context spans for the Preview view (top-left of the status bar). Chip + (when an image) zoom factor.
pub fn context(app: &App) -> Vec<Span<'static>> {
    // モードチップ(PREVIEW/IMAGE)は `status` が前置。ここでは画像の倍率のみ追記。
    let mut spans = Vec::new();
    if app.is_image_preview() {
        spans.push(Span::from(format!("  x{:.2}", app.image_zoom)).bold());
    }
    // PDF はページ位置を併記(例: 2/5)。総ページ数が判っているときだけ。
    if let Some((cur, total)) = app.pdf_page_indicator() {
        spans.push(Span::from(format!("  {cur}/{total}")).bold());
    }
    spans
}

/// The Preview view's `?` help section. Switches between image and (code/text/Markdown) variants.
/// **Edit the Preview help here**. The Markdown link operation rows (Tab/Enter) are here too.
pub fn help_sections(app: &App) -> Vec<crate::ui::help::HelpSection> {
    use crate::ui::help::HelpSection;
    let lang = app.lang;
    let l = |m| tr(lang, m);
    if app.is_git_diff_preview() {
        // Git 変更ハブ(`o`)→ Enter で開くファイル差分のプレビュー。
        return vec![HelpSection::new(l(crate::i18n::Msg::PreviewGitDiff))
            .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row(crate::ui::status::page_help(app), "")
            .row("x", l(crate::i18n::Msg::DiscardWholeFile))
            .row("q / Esc", l(crate::i18n::Msg::BackToGitView))];
    }
    if app.is_table_preview() {
        return vec![HelpSection::new(l(crate::i18n::Msg::PreviewTable))
            .row("h j k l / arrows", l(crate::i18n::Msg::TableMoveHelp))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("0 / $", l(crate::i18n::Msg::TableColsHelp))
            .row("y → c / r / C", l(crate::i18n::Msg::CopyHint))
            .row("y → f", l(crate::i18n::Msg::WkFull))
            .row("e", l(crate::i18n::Msg::EditExternal))
            .row("q / Esc", l(crate::i18n::Msg::BackToTree))];
    }
    if app.is_image_preview() {
        let mut sec = HelpSection::new(l(crate::i18n::Msg::PreviewImage))
            .row("+ / -", l(crate::i18n::Msg::Zoom))
            .row("0 / =", l(crate::i18n::Msg::ResetFit))
            .row("h j k l / arrows", l(crate::i18n::Msg::PanHint));
        if app.pdf_can_navigate() {
            sec = sec.row("J / K  ·  PageDown / PageUp", l(crate::i18n::Msg::HintPage));
        }
        return vec![sec
            .row("e", l(crate::i18n::Msg::EditExternal))
            .row("q / Esc", l(crate::i18n::Msg::BackToTree))];
    }
    vec![HelpSection::new(l(crate::i18n::Msg::PreviewTextMarkdown))
        .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
        .row("g / G", l(crate::i18n::Msg::TopBottom))
        .row("h / l / ← →", l(crate::i18n::Msg::HScroll))
        .row("0 / $", l(crate::i18n::Msg::LineStartEnd))
        .row("/  n / N", l(crate::i18n::Msg::SearchHint))
        .row("Tab / ⇧Tab", l(crate::i18n::Msg::FocusMdLink))
        .row("Enter", l(crate::i18n::Msg::OpenLinkHint))
        .row("e", l(crate::i18n::Msg::EditExternalEnv))
        .row(crate::ui::status::page_help(app), "")
        .row("q / Esc", l(crate::i18n::Msg::BackToTree))]
}

/// The Preview view's footer key hints. Switches by kind (image/Markdown/other text).
/// **Edit here to change the Preview footer**. Takes `&App`, so it can also depend on state.
pub fn footer_hints(app: &App) -> Vec<String> {
    let lang = app.lang;
    if app.is_table_preview() {
        return vec![
            hint(lang, "hjkl", crate::i18n::Msg::HintCell),
            hint(lang, "y", crate::i18n::Msg::CopyHint),
            hint(lang, "g/G", crate::i18n::Msg::HintEnds),
            hint(lang, "q", crate::i18n::Msg::GitBack),
            hint(lang, "?", crate::i18n::Msg::HintHelp),
            hint(lang, "e", crate::i18n::Msg::HintEdit),
            hint(lang, "[/]", crate::i18n::Msg::HintTab),
            hint(lang, "p", crate::i18n::Msg::HintPath),
        ];
    }
    if app.is_image_preview() {
        let mut v = vec![
            hint(lang, "+/-", crate::i18n::Msg::Zoom),
            hint(lang, "0/=", crate::i18n::Msg::HintFit),
            hint(lang, "hjkl", crate::i18n::Msg::HintPan),
        ];
        // 多ページ PDF のときだけページ送りを前寄りに出す(単ページ/poppler 無しでは出さない)。
        if app.pdf_can_navigate() {
            v.push(hint(lang, "J/K", crate::i18n::Msg::HintPage));
        }
        v.extend([
            hint(lang, "q", crate::i18n::Msg::GitBack),
            hint(lang, "?", crate::i18n::Msg::HintHelp),
            hint(lang, "e", crate::i18n::Msg::HintEdit),
            hint(lang, "[/]", crate::i18n::Msg::HintTab),
            hint(lang, "p", crate::i18n::Msg::HintPath),
        ]);
        return v;
    }
    if matches!(app.preview_kind, Some(PreviewKind::Markdown(_))) {
        // Markdown 固有のリンク操作(Tab フォーカス / Enter で開く)を前に出す。
        return vec![
            hint(lang, "jk", crate::i18n::Msg::Scroll),
            hint(lang, "Tab", crate::i18n::Msg::HintLink),
            hint(lang, "↵", crate::i18n::Msg::HintOpen),
            hint(lang, "q", crate::i18n::Msg::GitBack),
            hint(lang, "?", crate::i18n::Msg::HintHelp),
            hint(lang, "e", crate::i18n::Msg::HintEdit),
            hint(lang, "g/G", crate::i18n::Msg::HintEnds),
            hint(lang, "[/]", crate::i18n::Msg::HintTab),
            hint(lang, "p", crate::i18n::Msg::HintPath),
            page_hint(app),
        ];
    }
    // コード / 素テキスト。検索中は n/N(一致 cur/total)を前に出す。
    let mut v = vec![hint(lang, "jk", crate::i18n::Msg::Scroll)];
    if let Some((cur, total)) = app.search_status() {
        v.push(format!(
            "n/N:{}[{cur}/{total}]",
            tr(lang, crate::i18n::Msg::Match)
        ));
    } else if app.preview_search_query().is_some() {
        v.push(format!("n/N:{}", tr(lang, crate::i18n::Msg::Match)));
    }
    v.push(hint(lang, "/", crate::i18n::Msg::HintSearch));
    v.push(hint(lang, "q", crate::i18n::Msg::GitBack));
    v.push(hint(lang, "?", crate::i18n::Msg::HintHelp));
    v.push(hint(lang, "e", crate::i18n::Msg::HintEdit));
    v.push(hint(lang, "g/G", crate::i18n::Msg::HintEnds));
    v.push(hint(lang, "hl", crate::i18n::Msg::HintHscroll));
    v.push(hint(lang, "0/$", crate::i18n::Msg::HintLineEnds));
    v.push(hint(lang, "[/]", crate::i18n::Msg::HintTab));
    v.push(hint(lang, "p", crate::i18n::Msg::HintPath));
    v.push(page_hint(app));
    v
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // SVG/GIF を別スレッドで読み込み中: 生 XML や空きを出さず「読み込み中…」を表示する。
    if app.is_media_loading()
        && matches!(
            app.preview_kind,
            Some(
                PreviewKind::Image(_)
                    | PreviewKind::Svg(_)
                    | PreviewKind::Video(_)
                    | PreviewKind::Pdf(_)
            )
        )
    {
        render_media_loading(frame, app, area);
        return;
    }

    // 画像は専用パス: 枠を描いてから内側に StatefulImage を描画する。
    // バックエンド未初期化・デコード失敗 (app.image=None) はテキストにフォールバック。
    if matches!(app.preview_kind, Some(PreviewKind::Image(_))) {
        render_image(frame, app, area);
        return;
    }

    // SVG / 動画サムネ: ラスタ化・抽出に成功していれば画像経路で描画。失敗(image_src=None・
    // 端末非対応・外部ツール不在含む)時は下のテキスト経路へ流し、安全フォールバックを表示する
    // (設計原則#3「未対応は安全に」のグレースフル降格)。
    if matches!(
        app.preview_kind,
        Some(PreviewKind::Svg(_) | PreviewKind::Video(_) | PreviewKind::Pdf(_))
    ) && app.is_image_preview()
    {
        render_image(frame, app, area);
        return;
    }

    // CSV/TSV テーブル: 整列グリッド(列レインボー＋セルカーソル)で描画する(専用パス)。
    // パース失敗時は is_table_preview=false になり、下のテキスト経路で生 CSV へ安全降格する。
    if app.is_table_preview() {
        crate::ui::table::render(frame, app, area);
        return;
    }

    // GitDiff プレビュー: unified 差分を Zed 風着色で描画する(専用パス)。
    if app.is_git_diff_preview() {
        render_gitdiff(frame, app, area);
        return;
    }

    // 大きい Code/Text ファイルは less 風ウィンドウ読み(全読みしない)で描画する。
    if app.is_windowed() {
        render_windowed(frame, app, area);
        return;
    }

    // Markdown/Mermaid/コードは装飾済み (tui-markdown / mermaid-text / syntect) の行列を
    // 描画する専用パス。
    if matches!(
        app.preview_kind,
        Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_)) | Some(PreviewKind::Code(_))
    ) {
        render_decorated(frame, app, area);
        return;
    }

    let (body, is_text) = match &app.preview_kind {
        // テキスト(拡張子未登録)は実本文をそのまま描画。
        Some(PreviewKind::Text(p)) => (load_body(p, app.lang), true),
        Some(PreviewKind::Markdown(p))
        | Some(PreviewKind::Mermaid(p))
        | Some(PreviewKind::Code(p)) => {
            // ここには来ない (上で装飾パスへ分岐済み) が、網羅性のため安全側。
            (load_body(p, app.lang), true)
        }
        Some(PreviewKind::Image(p)) => (format!("[image] {}", p.display()), false),
        // ラスタ化に失敗/端末非対応の SVG。生 XML をテキストとして見せる(安全なフォールバック)。
        Some(PreviewKind::Svg(p)) => (load_body(p, app.lang), true),
        // サムネイル抽出不可(ffmpeg 系不在/端末非対応/失敗)の動画。対象ファイル＋導入ヒントを表示。
        Some(PreviewKind::Video(p)) => (
            format!(
                "{}\n{}",
                tr(app.lang, crate::i18n::Msg::VideoThumbUnavailable),
                p.display()
            ),
            false,
        ),
        // ラスタ化不可(pdftocairo/pdftoppm/qlmanage/sips いずれも不可)の PDF。対象ファイル＋ヒントを表示。
        Some(PreviewKind::Pdf(p)) => (
            format!(
                "{}\n{}",
                tr(app.lang, crate::i18n::Msg::PdfPreviewUnavailable),
                p.display()
            ),
            false,
        ),
        Some(PreviewKind::Command {
            path,
            template,
            render_as,
            detached,
        }) => (
            format!(
                "[command] {} :: {} (render_as={render_as:?}, detached={detached})",
                template,
                path.display()
            ),
            false,
        ),
        // テーブルは上の専用パスで描画済み。ここに来るのはパース失敗時=生 CSV/TSV をテキスト表示(安全降格)。
        Some(PreviewKind::Table { path, .. }) => (load_body(path, app.lang), true),
        Some(PreviewKind::CanNotPreview { ext }) => (format!("[can not preview: {ext}]"), false),
        // GitDiff は上の専用パスで描画済み(ここには来ない)。網羅性のため安全側。
        Some(PreviewKind::GitDiff(_)) => ("(git diff)".to_string(), false),
        None => ("(no preview)".to_string(), false),
    };

    let title = app
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());

    let wrap = is_text && app.cfg.ui.wrap;

    // 縦/横クランプの基準値を本文を move する前に算出。
    let logical_lines = body.lines().count();
    let max_line_cols = body.lines().map(|l| l.chars().count()).max().unwrap_or(0);

    let block = Block::bordered().title(title);
    let inner = block.inner(area); // 枠を除いた表示領域

    let mut para = Paragraph::new(body).block(block);
    if wrap {
        // trim:false で先頭空白を保持 (コード/テキストのインデントを崩さない)。
        para = para.wrap(Wrap { trim: false });
    }

    // 総表示行数: 折返し時は ratatui に計算させ、非折返し時は論理行数。
    let total_rows = if wrap {
        para.line_count(inner.width)
    } else {
        logical_lines
    };

    // 末尾を超えてスクロールしないようクランプ (最低1行は残す)。
    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.preview_scroll = app.preview_scroll.min(max_v);
    // ページ送り(PageUp/Down)の1ページ量に使うため、表示領域の高さを記録。
    app.preview_viewport = inner.height;

    // 横スクロール: 折返し時は不要 → 0。非折返し時は最長行が画面に収まる範囲まで。
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.preview_hscroll = app.preview_hscroll.min(max_h);

    let para = para.scroll((app.preview_scroll, app.preview_hscroll));
    frame.render_widget(para, area);
}

/// Decorated rendering for Markdown/Mermaid/code. Displays the lines generated by
/// tui-markdown / mermaid-text / syntect with full-screen scrolling. The decorated result is cached by (path, width) in App.
/// Scroll/wrap/clamp/page-step amounts reuse the same conventions as the text path.
fn render_decorated(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());

    let block = Block::bordered().title(title);
    let inner = block.inner(area); // 枠を除いた表示領域

    // 装飾行を取得 (mermaid は inner 幅でフィット済みなので、後段で折返しても罫線が崩れない)。
    // リンクを収集し、フォーカス中リンクは反転表示する (Markdown のみ。他種別は 0 件)。
    let lines = app.decorated_lines(inner.width);
    let lines = app.decorate_links(lines);
    let logical_lines = lines.len();
    let max_line_cols = lines.iter().map(|l| l.width()).max().unwrap_or(0);

    let wrap = app.cfg.ui.wrap;
    let mut para = Paragraph::new(Text::from(lines)).block(block);
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }

    // 総表示行数: 折返し時は ratatui に計算させ、非折返し時は論理行数。
    let total_rows = if wrap {
        para.line_count(inner.width)
    } else {
        logical_lines
    };
    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.preview_scroll = app.preview_scroll.min(max_v);
    app.preview_viewport = inner.height;

    // 横スクロール: 折返し時は不要。非折返し時のみ最長行が収まる範囲まで。
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.preview_hscroll = app.preview_hscroll.min(max_h);

    let para = para.scroll((app.preview_scroll, app.preview_hscroll));
    frame.render_widget(para, area);

    // Overlay block-level inline images (kitty graphics) over their reserved placeholder rows.
    // A partially-scrolled image is clipped to the viewport (only its visible band is drawn).
    overlay_inline_images(frame, app, inner);
}

/// Draw decoded inline Markdown images over their reserved rows. While an image is still decoding,
/// its placeholder rows remain visible. A partially-scrolled image is drawn clipped to the viewport
/// (its visible vertical band is cropped and encoded), so large images are not hidden while scrolling.
fn overlay_inline_images(frame: &mut Frame, app: &mut App, inner: Rect) {
    let placements = app.md_images();
    if placements.is_empty() {
        return;
    }
    let scroll = app.preview_scroll as i32;
    let top_bound = inner.y as i32;
    let bottom_bound = (inner.y + inner.height) as i32;
    for p in placements {
        let top = top_bound + p.line as i32 - scroll; // block's first row on screen
                                                      // Visible screen band for this image (clipped to the viewport top/bottom).
        let vis_top = top.max(top_bound);
        let vis_bottom = (top + p.rows as i32).min(bottom_bound);
        if vis_bottom <= vis_top {
            continue; // fully off-screen
        }
        let row_off = (vis_top - top) as u16; // image rows scrolled above the viewport
        let vis_rows = (vis_bottom - vis_top) as u16;
        let cols = p.cols.min(inner.width);
        if cols == 0 || vis_rows == 0 {
            continue;
        }
        let x = inner.x + (inner.width.saturating_sub(cols)) / 2;
        let target = Rect {
            x,
            y: vis_top as u16,
            width: cols,
            height: vis_rows,
        };
        app.ensure_md_image(&p.url, cols, p.rows, row_off, vis_rows);
        if let Some(proto) = app.md_image_proto(&p.url, cols, p.rows, row_off, vis_rows) {
            frame.render_widget(Image::new(proto), target);
        }
    }
}

/// GitDiff preview rendering. Displays the lines from `git::file_diff` Zed-style (gutter + change bar + colored body + row background)
/// with full-screen scrolling. If the diff is empty (clean), shows "(no changes)" in the center.
fn render_gitdiff(frame: &mut Frame, app: &mut App, area: Rect) {
    // Auto は内側幅で解決する。先に枠の内側幅を見積もる。
    let split = app.diff_is_split(Block::bordered().inner(area).width);
    let mode_tag = if split { " ⇆" } else { "" };
    let title = app
        .preview_path
        .clone()
        .map(|p| format!(" diff{mode_tag}: {} ", app.format_path(&p)))
        .unwrap_or_else(|| " diff ".to_string());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.preview_viewport = inner.height;

    let diff = app.git_diff_lines();
    if diff.is_empty() {
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
        return;
    }

    // 拡張子で本文を構文着色。テーマは設定のコードテーマを流用。
    let ext = app.current_preview_ext().to_string();
    let theme = app.cfg.ui.theme.code_theme.clone();
    let iw = inner.width as usize;
    // 横スクロール: 縦並びは Paragraph の横 offset、横並びは**各列の本文だけ**を内部でずらす
    // (ガター/区切りは固定)。どちらも h/l・0/$ で操作する同じ preview_hscroll を使う。
    let (lines, para_hscroll) = if split {
        let max_h = crate::preview::gitdiff::side_by_side_max_hscroll(&diff, iw) as u16;
        app.preview_hscroll = app.preview_hscroll.min(max_h);
        let lines = crate::preview::gitdiff::diff_lines_side_by_side(
            &diff,
            &ext,
            &theme,
            iw,
            app.preview_hscroll as usize,
        );
        (lines, 0)
    } else {
        let lines = crate::preview::gitdiff::diff_lines(&diff, &ext, &theme, iw);
        let max_h = lines
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(0)
            .saturating_sub(iw) as u16;
        app.preview_hscroll = app.preview_hscroll.min(max_h);
        (lines, app.preview_hscroll)
    };
    let total_rows = lines.len();

    // 縦スクロールは末尾を超えないようクランプ(diff は折返ししない=横は切り捨て表示)。
    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.preview_scroll = app.preview_scroll.min(max_v);

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((app.preview_scroll, para_hscroll));
    frame.render_widget(para, area);
}

/// less-style windowed rendering for large Code/Text files.
/// Reads only the visible window (from the start byte to the screen height) and colors Code with syntect (does not read the whole file).
/// Vertical scrolling is done by the "window cutout position", so Paragraph's vertical scroll stays 0. Only horizontal scroll is used.
fn render_windowed(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut title = match (app.preview_path.clone(), app.window_progress()) {
        (Some(p), Some(pct)) => format!(" {}  [{}%] ", app.format_path(&p), pct),
        (Some(p), None) => format!(" {} ", app.format_path(&p)),
        _ => " preview ".to_string(),
    };
    // progressive 待ち中はタイトルに「ハイライト中」を添える(本文は素テキストで即読める)。
    if app.is_highlight_pending() && !app.loading_is_indicator() {
        title.push_str(tr(app.lang, crate::i18n::Msg::Highlighting));
    }
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.preview_viewport = inner.height;

    // indicator 方式: cold な言語の初回は画面中央にスピナー(絵文字でなく点字の定番スピナー)を出す。
    // コンパイルは裏スレッドで走り、run ループが待機中にコマを進めるので**回り続ける**(フリーズ無し)。
    if app.is_highlight_pending() && app.loading_is_indicator() {
        frame.render_widget(block, area);
        render_spinner_line(
            frame,
            inner,
            app.spinner_glyph(),
            tr(app.lang, crate::i18n::Msg::Loading),
        );
        return;
    }

    let lines = app.windowed_lines(inner.height, inner.width);
    let max_line_cols = lines.iter().map(|l| l.width()).max().unwrap_or(0);

    let wrap = app.cfg.ui.wrap;
    let mut para = Paragraph::new(Text::from(lines)).block(block);
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }
    // 横スクロール: 折返し時は不要。非折返し時のみ窓内の最長行が収まる範囲まで。
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.preview_hscroll = app.preview_hscroll.min(max_h);
    // 縦は窓で切り出し済みなので 0。横のみスクロール。
    let para = para.scroll((0, app.preview_hscroll));
    frame.render_widget(para, area);
}

/// Shared component for a loading display that draws one line of "spinner  message" centered inside the frame.
/// Used both while waiting on code highlighting (indicator) and while loading SVG/GIF on a separate thread.
/// The spinner **keeps spinning** because the run loop advances it via `tick_spinner` while waiting (braille ⠋⠙⠹…).
fn render_spinner_line(frame: &mut Frame, inner: Rect, spinner: &str, msg: &str) {
    let y = inner.y + inner.height / 2;
    let line_area = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: 1,
    };
    let text = format!("{spinner}  {msg}");
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), line_area);
}

/// Display shown while loading SVG/GIF on a separate thread (frame + centered spinner + "loading…").
fn render_media_loading(frame: &mut Frame, app: &App, area: Rect) {
    let title = app
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " image ".to_string());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_spinner_line(
        frame,
        inner,
        app.spinner_glyph(),
        tr(app.lang, crate::i18n::Msg::Loading),
    );
}

/// Image preview rendering. Draws the frame + title, then draws the image inside.
/// Still images (PNG/JPG/SVG) use the async StatefulImage; GIF animations are drawn with the
/// Image widget using a synchronously-encoded Protocol (avoids the "draw unencoded → blank" churn for animations that change wholesale each frame).
/// When rendering is impossible (unsupported terminal / decode failure), falls back safely to a message.
fn render_image(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " image ".to_string());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lang = app.lang;
    let fallback = |frame: &mut Frame| {
        let msg = tr(lang, crate::i18n::Msg::ImageUnsupported);
        frame.render_widget(Paragraph::new(msg), inner);
    };

    // GIF: 同期エンコード済み Protocol を Image ウィジェットで原子的に描画(churn/未エンコード空き無し)。
    if app.is_gif_active() {
        let target = app.prepare_gif(inner);
        match (target, app.gif_protocol()) {
            (Some(target), Some(proto)) => frame.render_widget(Image::new(proto), target),
            _ => fallback(frame),
        }
        return;
    }

    // 静止画: 描画直前に (zoom, center, inner) から crop と表示矩形を確定。
    // z=1=フィット, 拡大で大きく, viewport を超えたら見切れ＋パン(中央寄せ)。別スレッドでリサイズ/エンコード。
    let target = app.prepare_image(inner);
    match (target, app.image.as_mut()) {
        (Some(target), Some(state)) => {
            // 最近傍(None)だと縮小でエイリアス・拡大でブロックノイズになり高解像度ソースでも粗く見える。
            // Lanczos3 で高品質リサイズ(リサイズ/エンコードは別スレッド=resize_worker なので UI を阻害しない)。
            let widget = StatefulImage::new().resize(Resize::Scale(Some(FilterType::Lanczos3)));
            frame.render_stateful_widget(widget, target, state);
        }
        _ => fallback(frame),
    }
}

/// Load the text body with a size cap and turn it into a display string.
/// A load failure does not crash; it falls back to a safe-side message.
fn load_body(path: &Path, lang: crate::i18n::Lang) -> String {
    match text::load(path) {
        Ok(content) => {
            let mut s = content.lines.join("\n");
            if content.truncated {
                s.push_str(tr(lang, crate::i18n::Msg::PreviewTruncated));
            }
            s
        }
        // `[can not preview: …]` マーカーは仕様の英語固定表記 (cf. CanNotPreview の <ext>)。
        Err(e) => format!("[can not preview: load failed] {e}"),
    }
}

#[cfg(all(test, feature = "git"))]
mod gitdiff_tests {
    use crate::app::App;
    use crate::config::Config;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use std::path::Path;

    fn init_repo(dir: &Path) {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        cfg.set_str("commit.gpgsign", "false").ok();
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    }

    /// Render the GitDiff preview of a modified file and verify (1) changed rows have red/green background cells,
    /// (2) both the deleted line content (beta) and the added line content (gamma) appear on screen.
    #[test]
    fn gitdiff_preview_renders_with_colored_rows() {
        let dir = std::env::temp_dir().join("konoma_ui_gitdiff_render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.rs");
        std::fs::write(&f, b"alpha\nbeta\ngamma_keep\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "init"]);
        // 1 行を変更(beta → gamma)。
        std::fs::write(&f, b"alpha\ngamma\ngamma_keep\n").unwrap();

        let canon = dir.canonicalize().unwrap();
        let mut app = App::new(canon.clone(), Config::default()).unwrap();
        app.open_git_diff(&canon.join("a.rs"));
        assert!(
            app.is_git_diff_preview(),
            "GitDiff プレビューに入っていない"
        );

        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();

        // 画面文字列(削除/追加の両方の行内容が出る)。
        let s: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(s.contains("beta"), "削除行(beta)が出ていない: {s:?}");
        assert!(s.contains("gamma"), "追加行(gamma)が出ていない");

        // 変更行に赤 or 緑の背景セルが少なくとも 1 つある(Zed 風着色の証拠)。
        let added_bg = Color::Rgb(20, 48, 28);
        let removed_bg = Color::Rgb(58, 24, 26);
        let has_green = buf.content().iter().any(|c| c.bg == added_bg);
        let has_red = buf.content().iter().any(|c| c.bg == removed_bg);
        assert!(has_green, "追加行の緑背景セルが無い");
        assert!(has_red, "削除行の赤背景セルが無い");
    }

    /// A file with no diff (clean) shows "(no changes)".
    #[test]
    fn gitdiff_clean_file_shows_no_changes() {
        let dir = std::env::temp_dir().join("konoma_ui_gitdiff_clean");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.rs");
        std::fs::write(&f, b"alpha\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "init"]);

        let canon = dir.canonicalize().unwrap();
        let mut app = App::new(canon.clone(), Config::default()).unwrap();
        app.open_git_diff(&canon.join("a.rs"));
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            s.contains("no changes") || s.contains("変更なし"),
            "クリーン表示が出ない: {s:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
