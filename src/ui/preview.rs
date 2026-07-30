// Preview rendering (full screen).
// Text kinds (Markdown / Code / Text fallback) read the real body and display it with full-screen scrolling.
// Image (M2) and external-command delegation (M2+) are still just kind summaries. Markdown's rich rendering (decorated/Mermaid) is M3.
//
// Scrolling:
//   - Vertical: clamped at draw time so it never scrolls past the end (the content line count and screen height are known here).
//   - Horizontal: used to see long lines when ui.wrap=false. Disabled (clamped to 0) while wrapping.

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
    // The mode chip (PREVIEW/IMAGE) is prepended by `status`. Here we only append the image zoom factor.
    let mut spans = Vec::new();
    if app.is_image_preview() {
        spans.push(Span::from(format!("  x{:.2}", app.tab.image_zoom)).bold());
    }
    // PDF also shows the page position (e.g. 2/5), only when the total page count is known.
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
        // The file-diff preview opened via Enter from the Git changes hub (`o`).
        return vec![HelpSection::new(l(crate::i18n::Msg::PreviewGitDiff))
            .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("n / N", l(crate::i18n::Msg::JumpChangeHelp))
            .row("f", l(crate::i18n::Msg::HintFollowScope))
            .row(crate::ui::status::page_help(app), "")
            .row("x", l(crate::i18n::Msg::DiscardWholeFile))
            .row("q / Esc", l(crate::i18n::Msg::BackToGitView))];
    }
    if app.is_table_preview() {
        return vec![HelpSection::new(l(crate::i18n::Msg::PreviewTable))
            .row("h j k l / arrows", l(crate::i18n::Msg::TableMoveHelp))
            .row("g / G", l(crate::i18n::Msg::TopBottom))
            .row("0 / $", l(crate::i18n::Msg::TableColsHelp))
            .row("/  n / N", l(crate::i18n::Msg::TableSearchHelp))
            .row("Enter", l(crate::i18n::Msg::TableCellViewHelp))
            .row("y → c / r / C", l(crate::i18n::Msg::CopyHint))
            .row("y → f", l(crate::i18n::Msg::WkFull))
            .row("Ctrl-n / Ctrl-p", l(crate::i18n::Msg::PreviewFileJumpHelp))
            .row("m / '", l(crate::i18n::Msg::PreviewBookmarkHint))
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
            .row("Ctrl-n / Ctrl-p", l(crate::i18n::Msg::PreviewFileJumpHelp))
            .row("m / '", l(crate::i18n::Msg::PreviewBookmarkHint))
            .row("e", l(crate::i18n::Msg::EditExternal))
            .row("q / Esc", l(crate::i18n::Msg::BackToTree))];
    }
    vec![HelpSection::new(l(crate::i18n::Msg::PreviewTextMarkdown))
        .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
        .row("g / G", l(crate::i18n::Msg::TopBottom))
        .row("h / l / ← →", l(crate::i18n::Msg::HScroll))
        .row("0 / $", l(crate::i18n::Msg::LineStartEnd))
        .row("/  n / N", l(crate::i18n::Msg::SearchHint))
        .row("v / V → y", l(crate::i18n::Msg::PreviewSelectHelp))
        .row("Y", l(crate::i18n::Msg::AtRefHelp))
        .row("R", l(crate::i18n::Msg::MdRawToggleHelp))
        .row("o", l(crate::i18n::Msg::HintOutline))
        .row("Tab / ⇧Tab", l(crate::i18n::Msg::FocusMdLink))
        .row("Enter", l(crate::i18n::Msg::OpenLinkHint))
        .row("Ctrl-t", l(crate::i18n::Msg::OpenLinkNewTabHelp))
        .row("+ / - / 0", l(crate::i18n::Msg::MermaidZoomHelp))
        .row("Space", l(crate::i18n::Msg::MdTaskToggleHelp))
        .row("Space / ↵", l(crate::i18n::Msg::HintDetailsToggle))
        .row("Ctrl-n / Ctrl-p", l(crate::i18n::Msg::PreviewFileJumpHelp))
        .row("m / '", l(crate::i18n::Msg::PreviewBookmarkHint))
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
            hint(lang, "↵", crate::i18n::Msg::HintViewCell),
            hint(lang, "/", crate::i18n::Msg::HintSearch),
            hint(lang, "y", crate::i18n::Msg::CopyHint),
            hint(lang, "g/G", crate::i18n::Msg::HintEnds),
            hint(lang, "C-n/p", crate::i18n::Msg::HintFileJump),
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
        // Show page paging up front only for multi-page PDFs (not for single-page or unknown page count).
        if app.pdf_can_navigate() {
            v.push(hint(lang, "J/K", crate::i18n::Msg::HintPage));
        }
        v.extend([
            hint(lang, "C-n/p", crate::i18n::Msg::HintFileJump),
            hint(lang, "q", crate::i18n::Msg::GitBack),
            hint(lang, "?", crate::i18n::Msg::HintHelp),
            hint(lang, "e", crate::i18n::Msg::HintEdit),
            hint(lang, "[/]", crate::i18n::Msg::HintTab),
            hint(lang, "p", crate::i18n::Msg::HintPath),
        ]);
        return v;
    }
    if matches!(app.tab.preview_kind, Some(PreviewKind::Markdown(_))) && !app.is_raw_source() {
        // Markdown (decorated view)-specific link/checkbox operations (Tab to focus / Enter to open /
        // Space to toggle — shown only for documents with a checkbox) + R to switch to source view.
        let mut v = vec![
            hint(lang, "jk", crate::i18n::Msg::Scroll),
            hint(lang, "Tab", crate::i18n::Msg::HintLink),
            hint(lang, "↵", crate::i18n::Msg::HintOpen),
            hint(lang, "C-t", crate::i18n::Msg::HintNewTab),
        ];
        // While a code block is focused, y→c copies that block (shows up in y's copy menu).
        if app.md_focused_code() {
            v.push(hint(lang, "y c", crate::i18n::Msg::HintCopyCode));
        }
        // While an inline mermaid diagram is focused: zoom in place (+/-); while zoomed, hjkl=pan.
        if app.focused_mermaid_ordinal().is_some() {
            v.push(hint(lang, "+/-", crate::i18n::Msg::Zoom));
            if app.fence_zoom_level() > 1.001 {
                v.push(hint(lang, "hjkl", crate::i18n::Msg::HintPan));
                v.push(hint(lang, "0", crate::i18n::Msg::HintFit));
            }
        }
        if app.md_has_tasks() || app.md_focused_details().is_some() {
            v.push(hint(lang, "Space", crate::i18n::Msg::HintToggle));
        }
        v.extend([
            hint(lang, "o", crate::i18n::Msg::HintOutline),
            hint(lang, "R", crate::i18n::Msg::HintRawSource),
            hint(lang, "C-n/p", crate::i18n::Msg::HintFileJump),
            hint(lang, "q", crate::i18n::Msg::GitBack),
            hint(lang, "?", crate::i18n::Msg::HintHelp),
            hint(lang, "e", crate::i18n::Msg::HintEdit),
            hint(lang, "g/G", crate::i18n::Msg::HintEnds),
            hint(lang, "[/]", crate::i18n::Msg::HintTab),
            hint(lang, "p", crate::i18n::Msg::HintPath),
            page_hint(app),
        ]);
        return v;
    }
    // Code / plain text. While searching, show n/N (match cur/total) up front.
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
    // Range-selection copy (v=char / V=line) is windowed (Code/Text/raw Markdown) only.
    if app.is_windowed() {
        v.push(hint(lang, "v/V", crate::i18n::Msg::PreviewSelectHelp));
        v.push(hint(lang, "Y", crate::i18n::Msg::WkAtRef));
    }
    // A path back into follow (F) — shows it can be resumed with one key after it stops.
    v.push(hint(lang, "F", crate::i18n::Msg::StFollow));
    // For a follow-opened diff: f = toggle range (since follow-start ⇄ full), only when
    // follow_diff_scope_msg is Some.
    if app.follow_diff_scope_msg().is_some() {
        v.push(hint(lang, "f", crate::i18n::Msg::HintFollowScope));
    }
    // For Markdown/Mermaid, R toggles decorated view ⇄ raw source view (the label switches with the current mode).
    if app.is_decorated_kind() {
        let msg = if app.is_md_raw() {
            crate::i18n::Msg::HintRendered
        } else {
            crate::i18n::Msg::HintRawSource
        };
        v.push(hint(lang, "R", msg));
    }
    // NOTE: the outline (`o`) is only meaningful for the *decorated* Markdown view (raw source has no
    // heading cache), so its footer hint lives in the decorated-Markdown branch above — not here.
    v.push(hint(lang, "C-n/p", crate::i18n::Msg::HintFileJump));
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
    // While an SVG/GIF/mermaid is loading on a separate thread: show "loading…" instead of raw XML
    // or a blank area. While an image is already displayable (PDF page paging, zoom's sharp
    // re-rasterization), keep showing the old image (replacing it with a full-screen spinner would
    // flicker on every zoom).
    if app.is_media_loading()
        && !app.is_image_preview()
        && matches!(
            app.tab.preview_kind,
            Some(
                PreviewKind::Image(_)
                    | PreviewKind::Svg(_)
                    | PreviewKind::Video(_)
                    | PreviewKind::Pdf(_)
                    | PreviewKind::Mermaid(_)
                    | PreviewKind::MermaidFence(_)
            )
        )
    {
        render_media_loading(frame, app, area);
        return;
    }

    // Images take a dedicated path: draw the frame, then draw StatefulImage inside it.
    // An uninitialized backend / decode failure (app.image=None) falls back to text.
    if matches!(app.tab.preview_kind, Some(PreviewKind::Image(_))) {
        render_image(frame, app, area);
        return;
    }

    // SVG / video thumbnail: draw via the image path if rasterization/extraction succeeded. On
    // failure (image_src=None, including an unsupported terminal or a missing external tool), fall
    // through to the text path below and show a safe fallback (design principle #3 "unsupported is
    // safe" — graceful degradation).
    if matches!(
        app.tab.preview_kind,
        Some(
            PreviewKind::Svg(_)
                | PreviewKind::Video(_)
                | PreviewKind::Pdf(_)
                | PreviewKind::Mermaid(_)
                | PreviewKind::MermaidFence(_)
        )
    ) && app.is_image_preview()
    {
        render_image(frame, app, area);
        return;
    }

    // CSV/TSV table: draw it as an aligned grid (column rainbow + cell cursor) (dedicated path).
    // On a parse failure is_table_preview becomes false and it safely degrades to raw CSV via the
    // text path below.
    if app.is_table_preview() {
        crate::ui::table::render(frame, app, area);
        return;
    }

    // GitDiff preview: draw the unified diff with Zed-style coloring (dedicated path).
    if app.is_git_diff_preview() {
        render_gitdiff(frame, app, area);
        return;
    }

    // Large Code/Text files are drawn with less-style windowed reads (not read in full).
    if app.is_windowed() {
        render_windowed(frame, app, area);
        return;
    }

    // Dedicated path for Markdown/Mermaid/code, drawing the already-decorated
    // (tui-markdown / mermaid-text / syntect) lines.
    if matches!(
        app.tab.preview_kind,
        Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_)) | Some(PreviewKind::Code(_))
    ) {
        render_decorated(frame, app, area);
        return;
    }

    let (body, is_text) = match &app.tab.preview_kind {
        // Text (unregistered extension) draws the real body as-is.
        Some(PreviewKind::Text(p)) => (load_body(p, app.lang), true),
        Some(PreviewKind::Markdown(p))
        | Some(PreviewKind::Mermaid(p))
        | Some(PreviewKind::Code(p)) => {
            // Unreachable (already branched to the decorated path above), but kept safe for exhaustiveness.
            (load_body(p, app.lang), true)
        }
        Some(PreviewKind::Image(p)) => (format!("[image] {}", p.display()), false),
        // An SVG that failed to rasterize / whose terminal is unsupported. Shows the raw XML as text (a safe fallback).
        Some(PreviewKind::Svg(p)) => (load_body(p, app.lang), true),
        // Rendering failed in the full-screen fence view (an unsupported diagram kind, etc.). Shows guidance (q returns to the md).
        Some(PreviewKind::MermaidFence(_)) => (
            tr(app.lang, crate::i18n::Msg::MermaidUnavailable).to_string(),
            false,
        ),
        // A video whose thumbnail couldn't be extracted (no ffmpeg family/unsupported terminal/failure). Shows the target file + an install hint.
        Some(PreviewKind::Video(p)) => (
            format!(
                "{}\n{}",
                tr(app.lang, crate::i18n::Msg::VideoThumbUnavailable),
                p.display()
            ),
            false,
        ),
        // A PDF that couldn't be rasterized (none of hayro/pdftocairo/pdftoppm/qlmanage/sips
        // worked, or the terminal is unsupported). Shows the target file + a hint.
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
        // Tables are already drawn via the dedicated path above. Reaching here means the parse
        // failed = show the raw CSV/TSV as text (safe degradation).
        Some(PreviewKind::Table { path, .. }) => (load_body(path, app.lang), true),
        // Archives are also already drawn via the dedicated path (is_table_preview) above. Reaching
        // here means listing failed (a corrupted file/unsupported format). Rather than dumping the
        // raw zip/tar byte stream as text, shows the target file + a hint (principle #3).
        Some(PreviewKind::Archive { path, .. }) => (
            format!(
                "{}\n{}",
                tr(app.lang, crate::i18n::Msg::ArchiveListUnavailable),
                path.display()
            ),
            false,
        ),
        Some(PreviewKind::CanNotPreview { ext }) => (format!("[can not preview: {ext}]"), false),
        // GitDiff is already drawn via the dedicated path above (unreachable here). Kept safe for exhaustiveness.
        Some(PreviewKind::GitDiff(_)) => ("(git diff)".to_string(), false),
        None => ("(no preview)".to_string(), false),
    };

    let title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());

    let wrap = is_text && app.cfg.ui.wrap;

    // Compute the clamp baselines for vertical/horizontal before moving `body`.
    let logical_lines = body.lines().count();
    let max_line_cols = body.lines().map(|l| l.chars().count()).max().unwrap_or(0);

    let block = Block::bordered().title(title);
    let inner = block.inner(area); // the display area with the frame excluded

    let mut para = Paragraph::new(body).block(block);
    if wrap {
        // trim:false preserves leading whitespace (does not break code/text indentation).
        para = para.wrap(Wrap { trim: false });
    }

    // Total display row count: let ratatui compute it while wrapping, otherwise use the logical line count.
    let total_rows = if wrap {
        para.line_count(inner.width)
    } else {
        logical_lines
    };

    // Clamp so it never scrolls past the end (at least 1 line remains).
    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.tab.preview_scroll = app.tab.preview_scroll.min(max_v);
    // Record the display area's height for use as the 1-page amount for page paging (PageUp/Down).
    app.tab.preview_viewport = inner.height;

    // Horizontal scroll: unneeded while wrapping → 0. Otherwise, up to where the longest line fits on screen.
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);

    let para = para.scroll((app.tab.preview_scroll, app.tab.preview_hscroll));
    frame.render_widget(para, area);
}

/// Decorated rendering for Markdown/Mermaid/code. Displays the lines generated by
/// tui-markdown / mermaid-text / syntect with full-screen scrolling. The decorated result is cached by (path, width) in App.
/// Scroll/wrap/clamp/page-step amounts reuse the same conventions as the text path.
fn render_decorated(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());

    let block = Block::bordered().title(title);
    let inner = block.inner(area); // the display area with the frame excluded

    // Ensure the decoration cache, and get the total display row count (wrap-inclusive) and the
    // longest line width (mermaid is already fitted to the inner width, so its rule lines don't
    // break even after wrapping downstream). The line bodies themselves are returned by md_slice
    // below, only for the visible range — no full-document clone / full reflow every frame.
    let (total_rows, max_line_cols) = app.md_layout(inner.width);
    let wrap = app.cfg.ui.wrap;

    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.tab.preview_scroll = app.tab.preview_scroll.min(max_v);
    app.tab.preview_viewport = inner.height;
    // Remember the wrapped-row total so `e` can map the scroll position back to an approximate source
    // line (preview_edit_line). Same value preview_scroll is clamped against, so the fraction lines up.
    app.md_view_rows = total_rows;

    // Horizontal scroll: unneeded while wrapping. Only while not wrapping, up to where the longest line fits.
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);

    // The visible slice (with focus already inverted) + the remaining scroll from the slice's
    // start. Wrapping is independent per line (ratatui's Wrap does not span lines), so the slice's
    // rendered result matches the whole document.
    let (lines, local_scroll) = app.md_slice(app.tab.preview_scroll, inner.height);
    let mut para = Paragraph::new(Text::from(lines)).block(block);
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }
    let para = para.scroll((local_scroll, app.tab.preview_hscroll));
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
    // A signature of the draw position + focus state (for change detection). Hash only the
    // on-screen rects of images **actually drawn in this frame**: while every image is off-screen,
    // record no signature = don't trigger a full redraw (placeholder debris cleanup) on every
    // scroll key over the text portion. Cleanup is only needed on the frames where a placeholder
    // did/does exist in the grid (appearing, moving, leaving). Trigger it not just on position
    // changes but also **on focus/zoom changes**: in Ghostty, the frame that draws the focus border
    // can disturb the compositing of an adjacent placeholder row, so the grid is rebuilt on the very
    // frame the state changes (an immediate fix).
    use std::hash::{Hash, Hasher};
    let mut sig = std::collections::hash_map::DefaultHasher::new();
    (inner.x, inner.y, inner.width, inner.height).hash(&mut sig);
    let focused_mermaid = app.focused_mermaid_ordinal();
    focused_mermaid.hash(&mut sig);
    ((app.fence_zoom_level() * 1000.0) as u64).hash(&mut sig);
    let mut drawn = 0usize;
    let scroll = app.tab.preview_scroll as i32;
    let top_bound = inner.y as i32;
    let bottom_bound = (inner.y + inner.height) as i32;
    for p in placements {
        let is_mermaid = crate::preview::markdown::is_mermaid_fence_url(&p.url);
        // Match focus using the **source ordinal** (fence_ord) carried by the placement. Counting
        // by draw order drifts from focused_mermaid_ordinal when a loading/text-degraded fence
        // exists upstream.
        let this_focused = is_mermaid && p.fence_ord.is_some() && focused_mermaid == p.fence_ord;
        // p.line is the (logical) index of a decorated line, while preview_scroll is the visual row
        // after wrapping. While wrapping, the image would shift up by exactly the upstream wrap
        // count (= a megacell would overlap the inverted caption line and the placeholder row would
        // become an ID-color bar), so always convert to a visual row before subtracting.
        let vis_line = app.md_visual_span(p.line).0 as i32;
        let top = top_bound + vis_line - scroll; // block's first row on screen
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
        // Reaching here = an image actually drawn in this frame. Record its on-screen rect into the signature.
        (
            p.url.as_str(),
            target.x,
            target.y,
            target.width,
            target.height,
        )
            .hash(&mut sig);
        drawn += 1;
        // kitty's placeholder row assumes it's "printed in a clean SGR state" (a megacell only
        // embeds fg=ID and does not clear reverse/dim). If a cell inherits the underlying text's
        // style, a reversed cell exposes an ID-color bar as its background, so the image region's
        // cells always have their **attributes and fg** reset to plain. The background color is
        // preserved: a transparent diagram is composited over the cell background, so clearing it
        // would float a panel in the terminal's default color — rather than the theme
        // background — behind the diagram.
        {
            let buf = frame.buffer_mut();
            for y in target.top()..target.bottom() {
                for x in target.left()..target.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        let bg = cell.style().bg;
                        let mut s = ratatui::style::Style::reset();
                        if let Some(bg) = bg {
                            s = s.bg(bg);
                        }
                        cell.set_style(s);
                    }
                }
            }
        }
        // For a focused fence diagram, draw the selection border **outside the image** (the caption
        // line is the top edge, the bottom margin row is the bottom edge, and the sides are blank
        // columns). Since kitty graphics overlays the image on top of the text, the border sits
        // outside the image region = it's always visible even on a real terminal. Only when the
        // whole block is on-screen (no partial border; when focus moves, md_focus_move scrolls the
        // whole block into the visible range).
        if this_focused {
            let btop = top - 1; // caption line
            let bbot = top + p.rows as i32 + 1; // the row after the bottom margin row (exclusive)
                                                // Border width = whichever is wider, the image width or the title width (+corners).
                                                // Fully covers the text layer's (centered) caption = the title never overflows
                                                // the border, and the text below never leaks past its right edge.
            let z = app.fence_zoom_level();
            let lang = app.lang;
            // The border title covers the text layer's caption (prevents overflow). The affordance is also i18n'd.
            let title = if z > 1.001 {
                format!(
                    " ◇ mermaid  x{z:.1} — {} ",
                    tr(lang, crate::i18n::Msg::MermaidPanAffordance)
                )
            } else {
                format!(
                    " ◇ mermaid — {}  {} ",
                    tr(lang, crate::i18n::Msg::MermaidCaption),
                    tr(lang, crate::i18n::Msg::MermaidZoomAffordance)
                )
            };
            let tw = Span::from(title.as_str()).width() as u16 + 2;
            let bw = (cols + 2).max(tw).min(inner.width);
            let bx = inner.x + (inner.width.saturating_sub(bw)) / 2;
            if btop >= top_bound && bbot <= bottom_bound {
                use ratatui::style::{Color as C, Modifier as M, Style as S};
                // A four-sided selection border (the caption line = top edge/title, the margin row
                // = bottom edge, the sides = blank columns outside the image). The border is
                // unrelated to the "colored bar" issue (see the vis_line conversion above and the
                // image region's Style::reset for that bar's root cause).
                let brect = Rect {
                    x: bx,
                    y: btop as u16,
                    width: bw,
                    height: (bbot - btop) as u16,
                };
                let border = Block::bordered()
                    .border_style(S::new().fg(C::Cyan))
                    .title(title)
                    .title_style(S::new().fg(C::Cyan).add_modifier(M::REVERSED));
                frame.render_widget(border, brect);
            }
        }
        // In-place zoom for a focused fence: keep the reserved area as-is and draw a zoomed crop
        // inside it (only when the whole block is on-screen; falls back to normal display while
        // partially scrolled).
        let zoomed =
            this_focused && app.fence_zoom_level() > 1.001 && row_off == 0 && vis_rows >= p.rows;
        // If the display size (mermaid_rows) is larger than the base raster, follow up to a higher density (even while not zoomed).
        if is_mermaid {
            app.ensure_md_fence_density(&p.url, cols, p.rows);
        }
        if zoomed {
            app.ensure_md_fence_zoom(&p.url, cols, p.rows);
            if let Some(proto) = app.md_fence_zoom_proto(&p.url, cols, p.rows) {
                frame.render_widget(Image::new(proto), target);
                continue;
            }
        }
        app.ensure_md_image(&p.url, cols, p.rows, row_off, vis_rows);
        if let Some(proto) = app.md_image_proto(&p.url, cols, p.rows, row_off, vis_rows) {
            frame.render_widget(Image::new(proto), target);
        }
    }
    // A frame that drew nothing at all is not recorded (None) = only when something was drawn up
    // until just before does the finish side fire cleanup once as a "departure", and it does not
    // fire again for further off-screen scrolling.
    if drawn > 0 {
        app.note_md_overlay(sig.finish());
    }
}

/// GitDiff preview rendering. Displays the lines from `git::file_diff` Zed-style (gutter + change bar + colored body + row background)
/// with full-screen scrolling. If the diff is empty (clean), shows "(no changes)" in the center.
fn render_gitdiff(frame: &mut Frame, app: &mut App, area: Rect) {
    // Auto is resolved by the inner width. Estimate the frame's inner width first.
    let split = app.diff_is_split(Block::bordered().inner(area).width);
    let mode_tag = if split { " ⇆" } else { "" };
    // The position `(2/5)` within the set of changed files (the current spot for n/N cycling; not shown if outside the set).
    let pos = app
        .diff_change_position()
        .map(|(i, n)| format!(" ({i}/{n})"))
        .unwrap_or_default();
    // For a follow-opened diff, make the "since follow-start / full" range explicit (prevents confusion since the boundary is invisible).
    let scope = app
        .follow_diff_scope_msg()
        .map(|m| format!(" · {}", tr(app.lang, m)))
        .unwrap_or_default();
    let title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" diff{mode_tag}: {}{pos}{scope} ", app.format_path(&p)))
        .unwrap_or_else(|| " diff ".to_string());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.tab.preview_viewport = inner.height;

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

    // Syntax-color the body by extension. Reuse the code theme from config.
    let ext = app.current_preview_ext().to_string();
    let theme = app.cfg.ui.theme.code_theme.clone();
    let iw = inner.width as usize;
    // Horizontal scroll: stacked layout uses Paragraph's horizontal offset; side-by-side shifts
    // **each column's body text only** internally (the gutter/separator stay fixed). Both use the
    // same preview_hscroll, operated with h/l·0/$.
    let (lines, para_hscroll) = if split {
        let max_h = crate::preview::gitdiff::side_by_side_max_hscroll(&diff, iw) as u16;
        app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);
        let lines = crate::preview::gitdiff::diff_lines_side_by_side(
            &diff,
            &ext,
            &theme,
            iw,
            app.tab.preview_hscroll as usize,
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
        app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);
        (lines, app.tab.preview_hscroll)
    };
    let total_rows = lines.len();

    // Clamp vertical scroll so it never scrolls past the end (a diff never wraps = horizontal is truncated).
    let max_v = total_rows.saturating_sub(inner.height as usize) as u16;
    app.tab.preview_scroll = app.tab.preview_scroll.min(max_v);

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((app.tab.preview_scroll, para_hscroll));
    frame.render_widget(para, area);
}

/// less-style windowed rendering for large Code/Text files.
/// Reads only the visible window (from the start byte to the screen height) and colors Code with syntect (does not read the whole file).
/// Vertical scrolling is done by the "window cutout position", so Paragraph's vertical scroll stays 0. Only horizontal scroll is used.
fn render_windowed(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut title = match (app.tab.preview_path.clone(), app.window_progress()) {
        (Some(p), Some(pct)) => format!(" {}  [{}%] ", app.format_path(&p), pct),
        (Some(p), None) => format!(" {} ", app.format_path(&p)),
        _ => " preview ".to_string(),
    };
    // While showing raw source for Markdown/Mermaid, make it explicit in the title (distinguishes it from the decorated view).
    if app.is_raw_source() {
        title.push_str(&format!(
            "· {} ",
            tr(app.lang, crate::i18n::Msg::HintRawSource)
        ));
    }
    // While waiting on progressive rendering, add "highlighting" to the title (the body is immediately readable as plain text).
    if app.is_highlight_pending() && !app.loading_is_indicator() {
        title.push_str(tr(app.lang, crate::i18n::Msg::Highlighting));
    }
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    app.tab.preview_viewport = inner.height;

    // Indicator style: on a cold language's first time, show a spinner (the classic braille spinner,
    // not an emoji) centered on screen. Compilation runs on a background thread, and the run loop
    // advances its frame while waiting, so it **keeps spinning** (no freeze).
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
    // While not wrapping, follow horizontal scroll so the 2D caret never goes off-screen (same
    // convention as the table). The caret column is derived from the rendered line's REVERSED span
    // start position (= the display column, gutter included).
    if !wrap {
        if let Some((caret_disp, caret_row)) = caret_display_col(&lines) {
            let w = inner.width as usize;
            let line_w = lines[caret_row].width();
            if line_w <= w {
                // If the caret line fits within the screen width, display it from the start (does not isolate a character on a short line).
                app.tab.preview_hscroll = 0;
            } else {
                // A long line that doesn't fit follows with minimal movement (places the caret at the edge).
                let h = app.tab.preview_hscroll as usize;
                if caret_disp < h {
                    app.tab.preview_hscroll = caret_disp as u16;
                } else if caret_disp >= h + w {
                    app.tab.preview_hscroll = (caret_disp + 1 - w) as u16;
                }
            }
        }
    }
    let mut para = Paragraph::new(Text::from(lines)).block(block);
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }
    // Horizontal scroll: unneeded while wrapping. Only while not wrapping, up to where the longest line within the window fits.
    let max_h = if wrap {
        0
    } else {
        max_line_cols.saturating_sub(inner.width as usize) as u16
    };
    app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);
    // Vertical is already cut out by the window, so 0. Only horizontal scrolls.
    let para = para.scroll((0, app.tab.preview_hscroll));
    frame.render_widget(para, area);
}

/// The `(display_column, row_index)` of the 2D caret in the rendered windowed lines, or None if it isn't drawn
/// (e.g. an empty line). The caret is the single span carrying the REVERSED modifier, so its start column is the
/// sum of the widths of the spans before it (gutter included). Used for non-wrap horizontal follow.
fn caret_display_col(lines: &[ratatui::text::Line<'static>]) -> Option<(usize, usize)> {
    use ratatui::style::Modifier;
    for (row, line) in lines.iter().enumerate() {
        let mut disp = 0usize;
        for span in &line.spans {
            if span.style.add_modifier.contains(Modifier::REVERSED) {
                return Some((disp, row));
            }
            disp += span.width();
        }
    }
    None
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
        .tab
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
        .tab
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

    // GIF: draw the synchronously pre-encoded Protocol atomically via the Image widget (no churn/unencoded blank).
    if app.is_gif_active() {
        let target = app.prepare_gif(inner);
        match (target, app.gif_protocol()) {
            (Some(target), Some(proto)) => frame.render_widget(Image::new(proto), target),
            _ => fallback(frame),
        }
        return;
    }

    // Still images: right before drawing, fix the crop and display rect from (zoom, center, inner).
    // z=1=fit, larger while zoomed in, cropped + pan once it exceeds the viewport (centered).
    let target = app.prepare_image(inner);
    // kitty terminal: konoma's own compressed-transfer (o=z) path. On a crop change, draw the
    // already-built KittyImage directly into the frame buffer (one transfer, then placeholder-only
    // afterward). The transfer volume drops to a fraction.
    if app.uses_kitty_image() {
        match (target, app.kitty_image_ref()) {
            (Some(target), Some(ki)) => ki.render(target, frame.buffer_mut()),
            _ => fallback(frame),
        }
        return;
    }
    // Other terminals (sixel/iterm2/halfblocks): ratatui-image's async StatefulImage.
    match (target, app.image.as_mut()) {
        (Some(target), Some(state)) => {
            // Nearest-neighbor (None) aliases when shrinking and produces block noise when
            // enlarging, looking coarse even from a high-resolution source. Use Lanczos3 for
            // high-quality resizing (resizing/encoding runs on a separate thread = resize_worker, so
            // the UI is not blocked).
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
        // The `[can not preview: …]` marker is a fixed English string by spec (cf. CanNotPreview's <ext>).
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
        // Change one line (beta → gamma).
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

        // The on-screen string (both the deleted and added line contents appear).
        let s: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(s.contains("beta"), "削除行(beta)が出ていない: {s:?}");
        assert!(s.contains("gamma"), "追加行(gamma)が出ていない");

        // A changed row has at least one red or green background cell (evidence of Zed-style coloring).
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
