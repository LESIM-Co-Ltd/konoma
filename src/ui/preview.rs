// Preview rendering (full screen).
// Text kinds (Markdown / Code / Text fallback) read the real body and display it with full-screen scrolling.
// Image (M2) and external-command delegation (M2+) are still just kind summaries. Markdown's rich rendering (decorated/Mermaid) is M3.
//
// Scrolling:
//   - Vertical: clamped at draw time so it never scrolls past the end (the content line count and screen height are known here).
//   - Horizontal: used to see long lines when ui.wrap=false. Disabled (clamped to 0) while wrapping.

use std::path::Path;

use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::Frame;
use ratatui_image::{FilterType, Image, Resize, StatefulImage};

use crate::app::{App, ScrollExtent};
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
        // The file-diff preview opened via Enter from the Git changes hub (`o`). The media diff's
        // side-by-side view and the binary-summary/computing body (`docs/FEATURE-MEDIA-DIFF.md`
        // §5/§6) both have nothing to scroll — `j/k`/`g/G`/the page-step row are all omitted for
        // either, mirroring the footer's own `App::diff_footer_is_media_or_summary` gate
        // ([[hint-shown-iff-key-acts]]).
        let media_or_summary = app.diff_footer_is_media_or_summary();
        let mut sec = HelpSection::new(l(crate::i18n::Msg::PreviewGitDiff));
        if !media_or_summary {
            sec = sec
                .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
                .row("g / G", l(crate::i18n::Msg::TopBottom));
        }
        if app.media_diff_showing_pictures() {
            sec = sec.row("s", l(crate::i18n::Msg::MediaLayoutCycleHelp));
            if app.media_diff_can_page() {
                sec = sec.row("J / K  ·  PageDown / PageUp", l(crate::i18n::Msg::HintPage));
            }
        }
        // Same gate as the footer's own `R` hint ([[hint-shown-iff-key-acts]]): omit the row for a
        // target with only one presentation, and word it for however many `R` actually cycles
        // through (`App::diff_view_help_hint`).
        if let Some(msg) = app.diff_view_help_hint() {
            sec = sec.row("R", l(msg));
        }
        sec = sec.row("n / N", l(crate::i18n::Msg::JumpChangeHelp));
        // [[hint-shown-iff-key-acts]]: `f` only *acts* on a follow-originated diff
        // (`App::toggle_follow_diff_scope`'s own gate is `is_git_diff_preview() &&
        // diff_follow_scope`) — `follow_diff_scope_msg` is `Some` under that exact same condition
        // (pre-existing bug: this row used to show unconditionally, for every diff).
        if app.follow_diff_scope_msg().is_some() {
            sec = sec.row("f", l(crate::i18n::Msg::HintFollowScope));
        }
        if !media_or_summary {
            sec = sec.row(crate::ui::status::page_help(app), "");
        }
        // `x` (discard) only *acts* on a backend that can write (`App::git_diff_start_discard`'s own
        // gate is `crate::vcs::caps(&self.tab.root).write`, flashing `VcsReadOnly` otherwise) — the
        // footer already drops it for a read-only backend (jj) via the very same predicate
        // (`ui/status.rs::mode_footer`'s `DiffMediaHintNoDiscard`/`DiffScrollNoDiscardHint*` arms);
        // this row used to show unconditionally here ([[hint-shown-iff-key-acts]]).
        if crate::vcs::caps(&app.tab.root).write {
            sec = sec.row("x", l(crate::i18n::Msg::DiscardWholeFile));
        }
        return vec![sec.row("q / Esc", l(crate::i18n::Msg::BackToGitView))];
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
        // While this image/PDF/SVG preview *is* the diff's own `Preview` representation
        // (`PerTab::preview_from_diff`), `R` returns to the diff instead — mirrors the text
        // preview's identical row ([[hint-shown-iff-key-acts]]: omitted otherwise, since an
        // ordinary image preview has no raw/rendered toggle for `R` to do anything with).
        if app.preview_is_diff_representation() {
            sec = sec.row("R", l(crate::i18n::Msg::HintReturnToDiff));
        }
        return vec![sec
            .row("Ctrl-n / Ctrl-p", l(crate::i18n::Msg::PreviewFileJumpHelp))
            .row("m / '", l(crate::i18n::Msg::PreviewBookmarkHint))
            .row("e", l(crate::i18n::Msg::EditExternal))
            .row("q / Esc", l(crate::i18n::Msg::BackToTree))];
    }
    let mut sec = HelpSection::new(l(crate::i18n::Msg::PreviewTextMarkdown))
        .row("j / k / ↑ ↓", l(crate::i18n::Msg::Scroll))
        .row("g / G", l(crate::i18n::Msg::TopBottom))
        .row("h / l / ← →", l(crate::i18n::Msg::HScroll))
        .row("0 / $", l(crate::i18n::Msg::LineStartEnd))
        .row("/  n / N", l(crate::i18n::Msg::SearchHint));
    // `v / V → y` (range selection) only works windowed (raw source / plain code/text) —
    // `preview_enter_visual` requires it. The decorated view still has `Y`, but it only ever
    // copies the whole-file `@path` there (no caret/selection to speak of on a Tab-focused item).
    if app.is_windowed() {
        sec = sec
            .row("v / V → y", l(crate::i18n::Msg::PreviewSelectHelp))
            .row("Y", l(crate::i18n::Msg::AtRefHelp));
    } else {
        sec = sec.row("Y", l(crate::i18n::Msg::AtRefPathHelp));
    }
    // While this preview *is* the diff's own `Preview` representation, `R` returns to the diff
    // instead of toggling raw source (`docs/FEATURE-MD-RENDERED-DIFF.md` §4).
    let r_help = if app.preview_is_diff_representation() {
        l(crate::i18n::Msg::HintReturnToDiff)
    } else {
        l(crate::i18n::Msg::MdRawToggleHelp)
    };
    vec![sec
        .row("R", r_help)
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
        // While this preview *is* the diff's own `Preview` representation, `R` returns to the
        // diff — mirrors the text preview's identical hint (`App::diff_preview_raw_hint`'s own
        // shape, [[hint-shown-iff-key-acts]]: omitted for an ordinary, non-diff image preview).
        if app.preview_is_diff_representation() {
            v.push(hint(lang, "R", crate::i18n::Msg::HintReturnToDiff));
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
        // Markdown (decorated view): `Tab` always cycles focus; everything else here is
        // focus-dependent (`app.md_focused_kind()`) — a hint is shown iff the key would actually
        // do what the label says right now, mirroring `md_activate_focused` /
        // `md_open_focused_link_new_tab` / the `Space` fixed key in main.rs exactly (see
        // `MdFocus`'s own doc comment). No focus at all → none of the `↵`/`C-t`/`Space` hints.
        use crate::app::MdFocus;
        let mut v = vec![
            hint(lang, "jk", crate::i18n::Msg::Scroll),
            hint(lang, "Tab", crate::i18n::Msg::HintFocus),
        ];
        match app.md_focused_kind() {
            Some(MdFocus::LocalLink) => {
                v.push(hint(lang, "↵", crate::i18n::Msg::HintOpen));
                v.push(hint(lang, "C-t", crate::i18n::Msg::HintNewTab));
            }
            Some(MdFocus::AnchorLink) => {
                v.push(hint(lang, "↵", crate::i18n::Msg::HintJump));
            }
            Some(MdFocus::ExternalLink) => {
                v.push(hint(lang, "↵", crate::i18n::Msg::HintBrowser));
            }
            Some(MdFocus::Task) | Some(MdFocus::Details) => {
                v.push(hint(lang, "Space/↵", crate::i18n::Msg::HintToggle));
            }
            // A code block has no Enter action; `y c` (via the copy leader) copies it instead.
            Some(MdFocus::CodeBlock) => {
                v.push(hint(lang, "y c", crate::i18n::Msg::HintCopyCode));
            }
            Some(MdFocus::MermaidFence) => {
                v.push(hint(lang, "↵", crate::i18n::Msg::HintFullScreen));
                v.push(hint(lang, "+/-", crate::i18n::Msg::Zoom));
                // hjkl/0 pan only once zoomed AND the diagram is fully on screen — mirrors
                // `fence_pan_motion`'s own gate exactly.
                if app.fence_zoom_level() > 1.001 && app.focused_fence_fully_visible() {
                    v.push(hint(lang, "hjkl", crate::i18n::Msg::HintPan));
                    v.push(hint(lang, "0", crate::i18n::Msg::HintFit));
                }
            }
            None => {}
        }
        // R normally goes decorated → raw source; while this preview *is* the diff's own `Preview`
        // representation, it instead returns to the diff (`App::diff_preview_raw_hint`, §4).
        let r_msg = app
            .diff_preview_raw_hint()
            .unwrap_or(crate::i18n::Msg::HintRawSource);
        v.extend([
            hint(lang, "o", crate::i18n::Msg::HintOutline),
            hint(lang, "R", r_msg),
            hint(lang, "/", crate::i18n::Msg::HintSearch),
            hint(lang, "F", crate::i18n::Msg::StFollow),
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
    // Short label on purpose: the footer is one line shared with every other hint, so the
    // explanatory wording (`PreviewSelectHelp`) belongs to the `?` help screen, not here.
    if app.is_windowed() {
        v.push(hint(lang, "v/V", crate::i18n::Msg::HintSelect));
        v.push(hint(lang, "Y", crate::i18n::Msg::WkAtRef));
    }
    // A path back into follow (F) — shows it can be resumed with one key after it stops.
    v.push(hint(lang, "F", crate::i18n::Msg::StFollow));
    // For a follow-opened diff: f = toggle range (since follow-start ⇄ full), only when
    // follow_diff_scope_msg is Some.
    if app.follow_diff_scope_msg().is_some() {
        v.push(hint(lang, "f", crate::i18n::Msg::HintFollowScope));
    }
    // For Markdown/Mermaid, R toggles decorated view ⇄ raw source view (the label switches with the
    // current mode) — *unless* this preview is itself the diff's own `Preview` representation
    // (`docs/FEATURE-MD-RENDERED-DIFF.md` §4), in which case R returns to the diff instead (checked
    // first, and for *any* windowed kind — not only Markdown/Mermaid — since a plain Code/Text
    // file's diff can reach its `Preview` representation too: `App::diff_preview_raw_hint`'s own
    // doc comment). [[hint-shown-iff-key-acts]]: exactly one of the two ever applies.
    if let Some(msg) = app.diff_preview_raw_hint() {
        v.push(hint(lang, "R", msg));
    } else if app.is_decorated_kind() {
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

// ---------------------------------------------------------------------------------------------
// Scroll position indicator (title label + border-column scrollbar).
//
// Applies to the three *scrolling* previews only — windowed text/code/raw Markdown, decorated
// Markdown, and the git diff. **Not** to image/PDF/video/SVG/full-screen-mermaid views: those cells
// carry kitty graphics Unicode placeholders, where drawing anything else over them exposes an
// image-ID-colored bar (a symptom this codebase has chased down three separate times), and they
// don't scroll anyway — zoom (`x1.6`) and PDF paging (`2/3`) already report position for them.
// ---------------------------------------------------------------------------------------------

/// Position label for a preview title, with vim's `%P` semantics: `All` when nothing is off-screen,
/// `Top`/`Bot` at the two ends, and a percentage of the **scrollable range** in between (so 0 % and
/// 100 % are actually reachable, unlike a fraction of the whole document).
fn scroll_label(lang: crate::i18n::Lang, e: ScrollExtent) -> String {
    use crate::i18n::Msg;
    if e.max == 0 {
        tr(lang, Msg::ScrollAll).to_string()
    } else if e.pos == 0 {
        tr(lang, Msg::ScrollTop).to_string()
    } else if e.pos >= e.max {
        tr(lang, Msg::ScrollBot).to_string()
    } else {
        format!("{}%", e.pos * 100 / e.max)
    }
}

/// The ` [Bot] ` marker, as a **right-aligned** title for the block's top border.
///
/// A separate title rather than a suffix on the path title, because a path long enough to fill the
/// border would otherwise push the marker off the end — and the position is exactly what you can't
/// afford to lose (it's the whole point). ratatui draws right-aligned titles *after* left-aligned
/// ones, so when the two collide it's the tail of the path that gives way, which is also the part
/// you can recover from elsewhere (`p` cycles the path style, and `i` shows the full path).
fn scroll_title(lang: crate::i18n::Lang, e: ScrollExtent) -> Line<'static> {
    Line::from(format!(" [{}] ", scroll_label(lang, e))).right_aligned()
}

/// Translate a `ScrollExtent` into ratatui's `ScrollbarState`.
///
/// `content_length` is the number of **scroll positions** (`max + 1`), not the content's total
/// size, and `viewport_content_length` is always set explicitly. That combination is what makes
/// `max_viewport_position` (ratatui's internal `content_length - 1 + viewport`) come out as the
/// whole content, which in turn makes the thumb exactly `viewport / whole` of the track and puts
/// its bottom edge on the track's last row at the last scroll position. Passing the total size as
/// `content_length` instead — the shape the widget's own doc example uses — leaves the thumb
/// provably short of the end (with 100 rows in a 20-row viewport it stops 4 rows above the
/// bottom), because that denominator counts one viewport too many.
fn scrollbar_state(e: ScrollExtent) -> ScrollbarState {
    // Scale the triple down (preserving ratios) before it reaches usize arithmetic: the windowed
    // reader measures in bytes, so `pos` can be gigabytes on a large file, and ratatui multiplies
    // it by the track length internally. Dividing all three by the same factor keeps
    // `pos == max` (thumb at the bottom) and `pos == 0` (thumb at the top) exact, since integer
    // division is monotonic and maps equal values to equal values.
    const CAP: u64 = 1 << 20;
    let span = e.max.saturating_add(e.viewport).max(1);
    let d = span.div_ceil(CAP).max(1);
    let (pos, max, viewport) = (e.pos / d, e.max / d, e.viewport / d);
    ScrollbarState::new((max + 1) as usize)
        .position(pos as usize)
        .viewport_content_length(viewport.max(1) as usize)
}

/// Draw the vertical scroll indicator **on top of the block's right border column**, so it consumes
/// no text column at all: the thumb `█` simply replaces part of the border's `│`.
///
/// `area` is the block's *outer* rect (the same one passed to `Block::bordered()`), and the vertical
/// margin of 1 keeps the thumb off the two corner glyphs. The track symbol is switched off so the
/// block's own border shows through as the track — a combination ratatui made possible on purpose
/// ("to make it easier to use with a block using block characters"). The `▲`/`▼` arrow heads are
/// switched off too: their East Asian Width is Ambiguous, so a CJK fallback font can render them
/// two cells wide — the exact trap already hit with `☐`/`☑`. `█` shares its width class with the
/// `▎`/`▌`/`│` glyphs konoma already draws, so it adds no new risk.
fn render_scrollbar(frame: &mut Frame, area: Rect, e: ScrollExtent) {
    // Two border rows plus at least one track row.
    if area.height < 3 || area.width == 0 {
        return;
    }
    let mut state = scrollbar_state(e);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .track_symbol(None)
        .begin_symbol(None)
        .end_symbol(None);
    frame.render_stateful_widget(
        bar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // While an SVG/GIF/mermaid is loading on a separate thread: show "loading…" instead of raw XML
    // or a blank area. While an image is already displayable (PDF page paging, zoom's sharp
    // re-rasterization), keep showing the old image (replacing it with a full-screen spinner would
    // flicker on every zoom).
    // A `detached` command never sets media_loading (it's a synchronous spawn-and-forget, not a
    // worker job — see `App::start_media_load`), so it never hits this branch; it's handled by the
    // final kind-summary match below instead.
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
                    | PreviewKind::Command { .. }
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
                | PreviewKind::Command { .. }
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
    // (konoma's own block-model renderer / mermaid-text / syntect) lines.
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
        // A PDF that couldn't be rasterized (neither hayro nor — on macOS, for page 1 —
        // qlmanage/sips worked, or the terminal is unsupported). Shows the target file + a hint.
        Some(PreviewKind::Pdf(p)) => (
            format!(
                "{}\n{}",
                tr(app.lang, crate::i18n::Msg::PdfPreviewUnavailable),
                p.display()
            ),
            false,
        ),
        // Reached only for `detached` (a one-line "opened externally" summary that stays on
        // screen the whole time you're on this preview) or a failed delegated command (image/text
        // decode/run failure — `app.command_error()`); a successful non-detached run is drawn via
        // the windowed/image paths above instead, and an in-flight one is caught by the
        // media-loading branch near the top of this function.
        Some(PreviewKind::Command {
            path,
            template,
            detached,
            ..
        }) => match app.command_error() {
            Some(reason) => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                (
                    format!(
                        "[can not preview: {ext}]\n{}{reason}",
                        tr(app.lang, crate::i18n::Msg::CommandPreviewFailed)
                    ),
                    false,
                )
            }
            None if *detached => (
                format!(
                    "{}{}",
                    tr(app.lang, crate::i18n::Msg::CommandOpenedExternally),
                    template
                ),
                false,
            ),
            // Defensive fallback (should be unreachable — a non-detached, non-failed Command with
            // no image and no windowed reader means the loading branch above should have caught
            // it): keep it safe rather than exhaustiveness-forcing a panic.
            None => (tr(app.lang, crate::i18n::Msg::Loading).to_string(), false),
        },
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
/// konoma's own block-model renderer / mermaid-text / syntect with full-screen scrolling. The decorated result is cached by (path, width) in App.
/// Scroll/wrap/clamp/page-step amounts reuse the same conventions as the text path.
fn render_decorated(frame: &mut Frame, app: &mut App, area: Rect) {
    render_decorated_body(frame, app, area, |app| {
        app.tab
            .preview_path
            .clone()
            .map(|p| format!(" {} ", app.format_path(&p)))
            .unwrap_or_else(|| " preview ".to_string())
    });
}

/// The diff's `Rendered` presentation (`docs/FEATURE-MD-RENDERED-DIFF.md` §1/§2): shares
/// `render_decorated`'s entire body (`App::ensure_md_cache`'s own `DecoratedSource::Diff` branch
/// makes the decoration cache itself carry real images/mermaid diagrams/math, not a text-only
/// fallback), differing only in the title — `diff ⟨rendered⟩: path (i/n) · scope` instead of the
/// plain path.
#[cfg(feature = "git")]
fn render_diff_rendered(frame: &mut Frame, app: &mut App, area: Rect) {
    render_decorated_body(frame, app, area, |app| {
        let pos = app
            .diff_change_position()
            .map(|(i, n)| format!(" ({i}/{n})"))
            .unwrap_or_default();
        let scope = app
            .follow_diff_scope_msg()
            .map(|m| format!(" · {}", tr(app.lang, m)))
            .unwrap_or_default();
        app.tab
            .preview_path
            .clone()
            .map(|p| format!(" diff ⟨rendered⟩: {}{pos}{scope} ", app.format_path(&p)))
            .unwrap_or_else(|| " diff ".to_string())
    });
}

/// The image/PDF/SVG side-by-side "diff" (`docs/FEATURE-MEDIA-DIFF.md` §1/§4/§6) — the `Rendered`
/// presentation's own body for a media-capable target (`App::diff_media_active`). Polls the media-
/// diff worker (`App::poll_media_diff`), lays the two sides out with `preview::media_diff::layout`,
/// and draws each side's caption + picture through the same inline-image machinery
/// `overlay_inline_images` uses for a Markdown document (`App::ensure_md_image`/`App::
/// md_image_proto`) — the pictures themselves already live in `md_image_cache` under their own
/// `media-diff://` keys by the time this runs (`App::apply_media_diff`). Degrades to the binary
/// summary line (§5) when there is no picker (images disabled/unsupported terminal).
#[cfg(feature = "git")]
fn render_gitdiff_media(frame: &mut Frame, app: &mut App, area: Rect) {
    let pos = app
        .diff_change_position()
        .map(|(i, n)| format!(" ({i}/{n})"))
        .unwrap_or_default();
    let scope = app
        .follow_diff_scope_msg()
        .map(|m| format!(" · {}", tr(app.lang, m)))
        .unwrap_or_default();
    let title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" diff ⟨side by side⟩: {}{pos}{scope} ", app.format_path(&p)))
        .unwrap_or_else(|| " diff ".to_string());
    let inner = Block::bordered().inner(area);
    app.tab.preview_viewport = inner.height;
    frame.render_widget(Block::bordered().title(title), area);

    let Some(PreviewKind::GitDiff(path)) = app.tab.preview_kind.clone() else {
        return;
    };

    // No picker at all (a terminal with no graphics protocol, or images disabled): §4's "画像を描
    // けない端末・設定" — the summary line, computed from the worker exactly like
    // `empty_diff_body_text`'s own binary-summary case (any raster target works here, since no
    // picture will ever actually be decoded to screen size; `(64, 64)` is a harmless placeholder).
    let Some(cell_px) = app.picker_cell_px() else {
        let text = match app.poll_media_diff(&path, app.diff_media_page(), (64, 64)) {
            None => tr(app.lang, crate::i18n::Msg::DiffComputing).to_string(),
            Some(outcome) => media_summary_line(app.lang, &outcome)
                .unwrap_or_else(|| tr(app.lang, crate::i18n::Msg::GitNoChanges).to_string()),
        };
        draw_media_centered(frame, inner, text);
        return;
    };
    let raster_px =
        media_diff_raster_px(app, inner).unwrap_or_else(|| (cell_px.0 * 40, cell_px.1 * 20));
    let page = app.diff_media_page();
    let mut outcome = app.poll_media_diff(&path, page, raster_px);
    // A landed `Ready` whose `Picture` cache_key no longer has an entry in `md_image_cache` (e.g.
    // evicted by `App::enter_preview`'s file-switch clear when some *other* preview target was
    // visited in between — `App::md_image_cache_contains`'s own doc comment has the detail on
    // which paths do/don't trigger this, `docs/FEATURE-MEDIA-DIFF.md` §4) is re-kicked rather than
    // silently drawn as nothing.
    if let Some(crate::app::MediaDiffOutcome::Ready {
        ref old, ref new, ..
    }) = outcome
    {
        let stale = |s: &crate::app::MediaDiffSide| matches!(s, crate::app::MediaDiffSide::Picture(p) if !app.md_image_cache_contains(&p.cache_key));
        if stale(old) || stale(new) {
            app.invalidate_media_diff();
            outcome = app.poll_media_diff(&path, page, raster_px);
        }
    }

    match outcome {
        None => draw_media_centered(
            frame,
            inner,
            tr(app.lang, crate::i18n::Msg::DiffComputing).to_string(),
        ),
        Some(crate::app::MediaDiffOutcome::Unavailable) => draw_media_centered(
            frame,
            inner,
            tr(app.lang, crate::i18n::Msg::GitNoChanges).to_string(),
        ),
        Some(ref outcome @ crate::app::MediaDiffOutcome::Summary { .. }) => {
            let text = media_summary_line(app.lang, outcome)
                .unwrap_or_else(|| tr(app.lang, crate::i18n::Msg::GitNoChanges).to_string());
            draw_media_centered(frame, inner, text);
        }
        Some(crate::app::MediaDiffOutcome::Ready {
            kind: _,
            base,
            same_bytes,
            old,
            new,
        }) => {
            let old_px = media_side_natural_px(&old);
            let new_px = media_side_natural_px(&new);
            let geom = crate::preview::media_diff::layout(
                old_px,
                new_px,
                cell_px,
                inner,
                app.media_diff_layout(),
            );
            draw_media_caption(
                frame,
                app,
                geom.old.caption,
                &old,
                MediaCaptionCtx {
                    is_old: true,
                    base,
                    identical: false,
                    page,
                },
            );
            draw_media_caption(
                frame,
                app,
                geom.new.caption,
                &new,
                MediaCaptionCtx {
                    is_old: false,
                    base,
                    identical: same_bytes,
                    page,
                },
            );
            draw_media_separator(frame, &geom);
            draw_media_side(frame, app, &geom.old, &old, true);
            draw_media_side(frame, app, &geom.new, &new, false);
        }
    }
}

/// A single centered line of text, for `render_gitdiff_media`'s non-`Ready` bodies (computing/
/// summary/unavailable) — mirrors `render_gitdiff_source`'s own empty-diff placement.
#[cfg(feature = "git")]
fn draw_media_centered(frame: &mut Frame, inner: Rect, text: String) {
    if inner.height == 0 {
        return;
    }
    let y = inner.y + inner.height / 2;
    let rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), rect);
}

/// A pane's own pixel size for `preview::media_diff::layout`'s sizing — `None` for anything but a
/// landed `Picture` (nothing to size a pane around).
///
/// `p.natural_px` — the picture's **intrinsic** size in the kind's own unit (raster image: real
/// header pixel dimensions; SVG: viewBox; PDF: page points) — never the decoded raster's own pixel
/// dimensions (`MediaDiffPictureDecoded::natural_px`'s own doc comment has the detail: a PDF page is
/// rasterized to a fixed longest side regardless of its point size, an SVG to fit the caller's
/// `raster_px` box regardless of its viewBox, and a GIF's frames may be decode-budget-downscaled —
/// none of those decoded pixel sizes are comparable across the two sides of one diff, which is
/// exactly what `layout`'s "shared scale" (`docs/FEATURE-MEDIA-DIFF.md` §1) depends on). The decoded
/// raster's own pixel size is never fed back into this layout step; instead, `App::poll_md_encode`
/// always resizes media-diff pictures with `Resize::Scale` (never `Fit`) to exactly the cell box
/// this layout decided, so a decoded-vs-intrinsic mismatch (the norm here, not the exception) can
/// never make `Widget::render` refuse to draw or draw at the wrong size.
#[cfg(feature = "git")]
fn media_side_natural_px(side: &crate::app::MediaDiffSide) -> Option<(u32, u32)> {
    match side {
        crate::app::MediaDiffSide::Picture(p) => Some(p.natural_px),
        _ => None,
    }
}

#[cfg(all(test, feature = "git"))]
mod media_side_natural_px_tests {
    use super::media_side_natural_px;
    use crate::app::{MediaDiffPicture, MediaDiffSide};
    use std::path::PathBuf;

    #[test]
    fn reads_natural_px() {
        let side = MediaDiffSide::Picture(MediaDiffPicture {
            natural_px: (612, 792),
            bytes: 1234,
            page_count: Some(3),
            cache_key: PathBuf::from("x"),
        });
        assert_eq!(media_side_natural_px(&side), Some((612, 792)));
    }

    #[test]
    fn non_picture_sides_have_no_natural_px() {
        assert_eq!(media_side_natural_px(&MediaDiffSide::Absent), None);
        assert_eq!(
            media_side_natural_px(&MediaDiffSide::Failed {
                reason: "x".to_string()
            }),
            None
        );
        assert_eq!(media_side_natural_px(&MediaDiffSide::PageMissing), None);
    }
}

/// The per-side bits `draw_media_caption` needs beyond `(rect, side)` itself — bundled into one
/// struct rather than four more function parameters (`clippy::too_many_arguments`).
#[cfg(feature = "git")]
struct MediaCaptionCtx {
    is_old: bool,
    base: crate::app::MediaBase,
    /// Whether to append "identical content" — only ever `true` on the **new** side (the caller
    /// never sets it for the old side, `render_gitdiff_media`'s own call sites).
    identical: bool,
    page: u32,
}

/// One side's caption row: "Before/After · base · WxH · size[ · p. N/M][ · identical]"
/// (`docs/FEATURE-MEDIA-DIFF.md` §1) — or, for a side with no picture, "Before/After · base" alone
/// (the placeholder message itself is drawn centered in the picture area by `draw_media_side`, not
/// squeezed into this one row). Truncated to the pane's own width by display width (CJK-safe).
#[cfg(feature = "git")]
fn draw_media_caption(
    frame: &mut Frame,
    app: &App,
    rect: Rect,
    side: &crate::app::MediaDiffSide,
    ctx: MediaCaptionCtx,
) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    let lang = app.lang;
    let mut parts = vec![
        tr(
            lang,
            if ctx.is_old {
                crate::i18n::Msg::MediaDiffBefore
            } else {
                crate::i18n::Msg::MediaDiffAfter
            },
        )
        .to_string(),
        if ctx.is_old {
            tr(lang, crate::app::App::media_base_msg(ctx.base)).to_string()
        } else {
            tr(lang, app.media_new_side_msg()).to_string()
        },
    ];
    if let crate::app::MediaDiffSide::Picture(p) = side {
        parts.push(format!("{}×{}", p.natural_px.0, p.natural_px.1));
        parts.push(crate::fileops::human_size(p.bytes));
        if let Some(pc) = p.page_count {
            parts.push(format!("p. {}/{pc}", ctx.page));
        }
    }
    if ctx.identical {
        parts.push(tr(lang, crate::i18n::Msg::MediaDiffIdentical).to_string());
    }
    let text = crate::ui::status::truncate_display(&parts.join(" · "), rect.width as usize);
    frame.render_widget(Paragraph::new(text).dim(), rect);
}

/// The 1-cell separator between the two panes — `│` for side-by-side, `─` for stacked
/// (`docs/FEATURE-MEDIA-DIFF.md` §1).
#[cfg(feature = "git")]
fn draw_media_separator(frame: &mut Frame, geom: &crate::preview::media_diff::MediaDiffGeometry) {
    use crate::preview::media_diff::MediaDiffOrientation;
    let rect = geom.separator;
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let ch = match geom.orientation {
        MediaDiffOrientation::Side => "│",
        MediaDiffOrientation::Stack => "─",
    };
    let line = ch.repeat(rect.width as usize);
    for y in rect.y..rect.y + rect.height {
        let row = Rect {
            x: rect.x,
            y,
            width: rect.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line.clone()).dim(), row);
    }
}

/// One side's picture area: draws the decoded picture (via the Markdown inline-image machinery,
/// keyed on the landed `media-diff://` cache key) into `pane.image` when there is one, or a
/// centered placeholder message for `Absent`/`Failed`/`PageMissing` into `pane.area`
/// (`docs/FEATURE-MEDIA-DIFF.md` §1/§7).
///
/// The placeholder is centered in `pane.area` — the whole area below the caption — **not**
/// `pane.image`: `image` is always zero-sized for these three variants (`layout`'s own contract,
/// `MediaDiffPane`'s doc comment), so using it here (as an earlier version did) meant the
/// placeholder's own zero-size guard discarded it before ever drawing — a real bug (an untracked
/// file's old side, a deleted file's new side, and a page beyond one side's own PDF page count all
/// silently rendered nothing but the caption).
#[cfg(feature = "git")]
fn draw_media_side(
    frame: &mut Frame,
    app: &mut App,
    pane: &crate::preview::media_diff::MediaDiffPane,
    side: &crate::app::MediaDiffSide,
    is_old: bool,
) {
    match side {
        crate::app::MediaDiffSide::Picture(p) => {
            let rect = pane.image;
            if rect.width == 0 || rect.height == 0 {
                return;
            }
            let key = p.cache_key.to_string_lossy().to_string();
            app.ensure_md_image(&key, rect.width, rect.height, 0, rect.height);
            if let Some(img) = app.md_image_proto(&key, rect.width, rect.height, 0, rect.height) {
                img.render(rect, frame.buffer_mut());
            }
        }
        crate::app::MediaDiffSide::Absent => {
            let msg = if is_old {
                crate::i18n::Msg::MediaDiffAbsentOld
            } else {
                crate::i18n::Msg::MediaDiffAbsentNew
            };
            draw_media_centered(frame, pane.area, tr(app.lang, msg).to_string());
        }
        crate::app::MediaDiffSide::Failed { reason } => {
            draw_media_centered(
                frame,
                pane.area,
                format!(
                    "{}: {reason}",
                    tr(app.lang, crate::i18n::Msg::MediaDiffCannotDisplay)
                ),
            );
        }
        crate::app::MediaDiffSide::PageMissing => {
            draw_media_centered(
                frame,
                pane.area,
                tr(app.lang, crate::i18n::Msg::MediaDiffPageMissing).to_string(),
            );
        }
    }
}

/// The shared body both `render_decorated` and `render_diff_rendered` are thin wrappers around:
/// `md_layout` → `md_slice` → wrap → scrollbar → `overlay_inline_images`. `title_for` supplies only
/// the path-title text; the scroll-position suffix (`scroll_title`) is appended identically either
/// way.
fn render_decorated_body(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    title_for: impl Fn(&App) -> String,
) {
    // The inner rect is independent of the title (the title is drawn on the top border row), so it
    // can be measured before the title exists — which it must be, because the title now carries the
    // scroll label and that isn't known until the layout below has run.
    let inner = Block::bordered().inner(area); // the display area with the frame excluded

    // Ensure the decoration cache, and get the total display row count (wrap-inclusive) and the
    // longest line width (mermaid is already fitted to the inner width, so its rule lines don't
    // break even after wrapping downstream). The line bodies themselves are returned by md_slice
    // below, only for the visible range — no full-document clone / full reflow every frame.
    let (total_rows, max_line_cols) = app.md_layout(inner.width);
    let wrap = app.cfg.ui.wrap;

    // A follow jump into a decorated Markdown document, or a fresh `Rendered` diff open/cycle,
    // requested a scroll to its first change-gutter mark (`docs/FEATURE-MD-RENDERED-DIFF.md` §2/§3)
    // but couldn't compute the display row itself at jump time (it depends on this very width) —
    // the cache above now has one, so this is the first draw where it's actually knowable. A no-op
    // when there is no mark to scroll to, or when the reservation isn't for the path on screen right
    // now (the reservation is simply consumed to nothing either way — `take_diff_scroll_pending_for`).
    //
    // Skipped entirely while the block-diff behind this cache is still computing on a separate
    // thread (`docs/STATUS.md` ★未修正 item 4) — `App::md_diff_pending_for_current` alone covers
    // both shapes that takes (its own doc comment has the detail): the `Rendered` presentation's
    // "computing…" placeholder body only ever exists exactly when this is `true` for the on-screen
    // path, and an ordinary preview's still-computing gutter has no placeholder at all, so this is
    // the only signal for it. A second, narrower placeholder-only check used to sit alongside this
    // one; pre-merge review of PR #21 found it could never be `true` without this one also being
    // `true`, so it was dropped as redundant. Consuming the request while either kind is still
    // computing would lose it for the *real* frame that lands once the worker finishes.
    if !app.md_diff_pending_for_current()
        && app
            .tab
            .preview_path
            .clone()
            .is_some_and(|p| app.take_diff_scroll_pending_for(&p))
    {
        if let Some(row) = app.md_first_diff_mark_row() {
            app.scroll_preview_to_row_with_context(row);
        }
    }

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

    // Everything here counts wrapped display rows, the unit this view scrolls in.
    let extent = ScrollExtent::new(
        app.tab.preview_scroll as u64,
        max_v as u64,
        inner.height as u64,
    );
    let title = title_for(app);
    let block = Block::bordered()
        .title(title)
        .title(scroll_title(app.lang, extent));

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
    // Before the inline images: the bar lives on the border column, which is outside every image
    // rect (they are centered inside `inner`), so the two never share a cell in either order.
    render_scrollbar(frame, area, extent);

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
    // `App::md_slice` prepends a 1-cell change-gutter column to every line whenever the current
    // cache carries any mark at all (`App::md_gutter_active` — `docs/FEATURE-MD-RENDERED-DIFF.md`
    // §2/§3), which shifts the whole text body one column right; `p.col` was measured against the
    // *un-shifted* layout (`render_doc`'s own placeholder text has no notion of a gutter), so every
    // placement needs the identical +1 to land back under its own placeholder row.
    let gutter = if app.md_gutter_active() { 1 } else { 0 };
    // One pass = one "frame" for inline-image slot bookkeeping. Every `ensure_md_image` below
    // stamps this number onto the slot it wants, which is how `reserve_proto_slot` tells a slot the
    // picture on screen right now needs (never recycle it) from one left over at an old size.
    app.begin_md_image_frame();
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
    // Resolved once per frame, not per image: the focus frame below reads it, and re-parsing two
    // config strings inside the draw loop would be needless work on every redraw.
    let image_align = app.cfg.ui.md_block_aligns().image;
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
        // `p.col` is the offset `render_doc` baked into the placeholder text (see
        // `ImagePlacement.col`'s own doc comment): at the top level this is measured from the same
        // origin as `inner.x` — the only call site (`App::ensure_md_cache`, via
        // `App::md_layout(inner.width)`) always renders at `width == inner.width`. Several images
        // sharing one row (the badge-row idiom) are packed left to right by `render_image_group`, so
        // `p.col` already places each one at its own column within that shared row; clamped against
        // `inner.width - cols` the same defensive way the old always-centered formula was implicitly
        // bounded, in case a placement's own `cols` alone already exceeds the pane (a very narrow pane
        // or an unusually wide single image).
        let x = inner.x + gutter + p.col.min(inner.width.saturating_sub(cols));
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
            // The frame is placed by the **same** rule the diagram itself was
            // (`[ui] md_image_align`, via `markdown::mermaid_diagram_col` in the renderer), never by
            // a second centering formula of its own — see `mermaid_focus_border_x`. For the default
            // `center` this is the identical `(inner.width - bw) / 2` this line always computed;
            // `mermaid_diagram_col` keeps one column free on the aligned side for a left/right
            // diagram, so the frame never lands under the picture.
            let bx = inner.x
                + crate::preview::markdown::mermaid_focus_border_x(image_align, inner.width, bw);
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
        // On a kitty terminal this is konoma's own compressed transmit on a fixed image id (so a
        // GIF re-encoding every frame *replaces* its picture in the terminal instead of piling up
        // one per frame); everywhere else it is the ratatui-image protocol, unchanged.
        if zoomed {
            app.ensure_md_fence_zoom(&p.url, cols, p.rows);
            if let Some(img) = app.md_fence_zoom_proto(&p.url, cols, p.rows) {
                img.render(target, frame.buffer_mut());
                continue;
            }
        }
        app.ensure_md_image(&p.url, cols, p.rows, row_off, vis_rows);
        if let Some(img) = app.md_image_proto(&p.url, cols, p.rows, row_off, vis_rows) {
            img.render(target, frame.buffer_mut());
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
    // The `Rendered` presentation (`docs/FEATURE-MD-RENDERED-DIFF.md` §2) is a wholly separate
    // draw path — decorated blocks, not the unified/split diff `render_gitdiff_source` below draws
    // — so it's dispatched away first, the same way `ui/preview.rs::render`'s own top-level match
    // routes each preview kind to its own function. Whether `Rendered` is even readable and has
    // anything to mark is decided up front, at the moment `tab.diff_view` is *set*
    // (`App::apply_diff_view`, called from every site that decides the presentation — §5's "描画中に
    // 状態を変えて表示に頼る設計をやめる"), not discovered mid-draw here: `App::md_layout`/
    // `ensure_md_cache` only ever *read* `tab.diff_view` now, so there is no same-frame fallback
    // left to check for (a previous version of this function had one, for a case that no longer
    // exists — `App::ensure_md_cache`'s own doc comment).
    #[cfg(feature = "git")]
    if app.diff_rendered_active() {
        if app.diff_media_active() {
            render_gitdiff_media(frame, app, area);
        } else {
            render_diff_rendered(frame, app, area);
        }
        return;
    }
    render_gitdiff_source(frame, app, area);
}

/// The classic unified/split diff — `render_gitdiff`'s pre-existing body, extracted so
/// `render_diff_rendered` can fall back into it directly (same frame) when the `Rendered`
/// presentation's own cache build has to give up and round down to `Source` (§5).
fn render_gitdiff_source(frame: &mut Frame, app: &mut App, area: Rect) {
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
    // The title carries the scroll label, which isn't known until the row count below has been
    // computed, so build the title last. The inner rect doesn't depend on it (the title is drawn on
    // the top border row).
    let inner = Block::bordered().inner(area);
    let titled = |app: &App, e: ScrollExtent| {
        let title = app
            .tab
            .preview_path
            .clone()
            .map(|p| format!(" diff{mode_tag}: {}{pos}{scope} ", app.format_path(&p)))
            .unwrap_or_else(|| " diff ".to_string());
        Block::bordered()
            .title(title)
            .title(scroll_title(app.lang, e))
    };
    app.tab.preview_viewport = inner.height;

    let diff = app.git_diff_lines();
    if diff.is_empty() {
        // Nothing to scroll: `All` + a full-length thumb, exactly like any content that fits.
        let extent = ScrollExtent::new(0, 0, inner.height as u64);
        frame.render_widget(titled(app, extent), area);
        let msg = empty_diff_body_text(app, inner);
        let y = inner.y + inner.height / 2;
        let line_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), line_area);
        render_scrollbar(frame, area, extent);
        return;
    }

    // Syntax-color the body by extension. Reuse the code theme from config.
    let ext = app.current_preview_ext().to_string();
    let theme = app.cfg.ui.theme.code_theme.clone();
    let iw = inner.width as usize;
    let ih = inner.height as usize;
    // Horizontal scroll: stacked layout uses Paragraph's horizontal offset; side-by-side shifts
    // **each column's body text only** internally (the gutter/separator stay fixed). Both use the
    // same preview_hscroll, operated with h/l·0/$.
    //
    // Only the rows actually on screen are ever syntax-highlighted (`highlight_line_by_ext`, via
    // `diff_lines_range`/`diff_lines_side_by_side_range`) — that's the expensive step (measured
    // ~60µs/row; ~400–760ms for a real 6,500-row diff when the whole document was highlighted on
    // every keypress, vs. a ~60ms render budget). The total row count (needed to clamp vertical
    // scroll *before* slicing which rows to render) and the horizontal scroll ceiling both come
    // from highlight-free layout passes (`side_by_side_row_count`/`unified_max_hscroll`/
    // `side_by_side_max_hscroll`), so per-frame cost stays O(viewport) instead of O(diff size).
    let (lines, para_hscroll, max_v) = if split {
        let max_h = crate::preview::gitdiff::side_by_side_max_hscroll(&diff, iw) as u16;
        app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);
        let total_rows = crate::preview::gitdiff::side_by_side_row_count(&diff, &ext);
        let max_v = total_rows.saturating_sub(ih) as u16;
        app.tab.preview_scroll = app.tab.preview_scroll.min(max_v);
        let lines = crate::preview::gitdiff::diff_lines_side_by_side_range(
            &diff,
            &ext,
            &theme,
            iw,
            app.tab.preview_hscroll as usize,
            app.tab.preview_scroll as usize,
            ih,
        );
        (lines, 0, max_v)
    } else {
        let total_rows = diff.len(); // one DiffLine = one unified row, headers included
        let max_v = total_rows.saturating_sub(ih) as u16;
        app.tab.preview_scroll = app.tab.preview_scroll.min(max_v);
        let max_h = crate::preview::gitdiff::unified_max_hscroll(&diff, iw) as u16;
        app.tab.preview_hscroll = app.tab.preview_hscroll.min(max_h);
        let lines = crate::preview::gitdiff::diff_lines_range(
            &diff,
            &ext,
            &theme,
            iw,
            app.tab.preview_scroll as usize,
            ih,
        );
        (lines, app.tab.preview_hscroll, max_v)
    };

    // Vertical scroll is already applied by slicing to the visible window above (unlike before,
    // when the full document was handed to Paragraph and Paragraph's own scroll clipped it).
    let extent = ScrollExtent::new(app.tab.preview_scroll as u64, max_v as u64, ih as u64);
    let para = Paragraph::new(Text::from(lines))
        .block(titled(app, extent))
        .scroll((0, para_hscroll));
    frame.render_widget(para, area);
    render_scrollbar(frame, area, extent);
}

/// Round `px` up to the next multiple of `step` (never below `step`) — used to quantize the media-
/// diff raster target so a one-pixel terminal resize doesn't kick a fresh SVG/PDF raster request
/// every frame (`docs/FEATURE-MEDIA-DIFF.md` §3's own note on this).
fn quantize_px(px: u32, step: u32) -> u32 {
    px.div_ceil(step).max(1) * step
}

/// The pixel box `render_gitdiff_media` asks the media-diff worker to rasterize an SVG/PDF side
/// into — the whole diff body's own box (`inner`), not each pane's smaller one: which orientation
/// (and therefore how big each pane actually is) the sides end up laid out in depends on their own
/// natural size, which isn't known until *after* they're decoded, so this deliberately over-asks
/// using the full body as an upper bound rather than guessing the eventual split first (the same
/// "never enlarge, only ever shrink to fit" contract `preview::media_diff::layout` gives the result
/// means over-asking here costs a slightly bigger raster, never a wrong-looking one). `None` when
/// there is no picker at all (`App::picker_cell_px`) — `docs/FEATURE-MEDIA-DIFF.md` §4's "画像を描
/// けない端末・設定" degrades to the binary summary line before this is ever called.
fn media_diff_raster_px(app: &App, inner: Rect) -> Option<(u32, u32)> {
    let (cw, ch) = app.picker_cell_px()?;
    let w = quantize_px((inner.width as u32).max(1) * cw.max(1), 64);
    let h = quantize_px((inner.height as u32).max(1) * ch.max(1), 64);
    Some((w, h))
}

/// The binary-summary-line text (`docs/FEATURE-MEDIA-DIFF.md` §5) for a landed media-diff outcome —
/// `None` when the outcome doesn't call for one at all: a genuine "(no changes)" (either shape with
/// `same_bytes`), or `Unavailable` (the worker panicked/gave up — the caller keeps its own existing
/// "(no changes)" text rather than claiming a binary change it can't actually back up with sizes).
fn media_summary_line(
    lang: crate::i18n::Lang,
    outcome: &crate::app::MediaDiffOutcome,
) -> Option<String> {
    use crate::app::{MediaDiffOutcome, MediaDiffSide};
    let (same_bytes, old_len, new_len) = match outcome {
        MediaDiffOutcome::Summary {
            same_bytes,
            old_len,
            new_len,
            ..
        } => (*same_bytes, *old_len, *new_len),
        MediaDiffOutcome::Ready {
            same_bytes,
            old,
            new,
            ..
        } => {
            let side_len = |s: &MediaDiffSide| match s {
                MediaDiffSide::Picture(p) => Some(p.bytes),
                _ => None,
            };
            (*same_bytes, side_len(old), side_len(new))
        }
        MediaDiffOutcome::Unavailable => return None,
    };
    if same_bytes {
        return None;
    }
    let fmt = |n: Option<u64>| match n {
        Some(b) => crate::fileops::human_size(b),
        None => tr(lang, crate::i18n::Msg::MediaDiffNone).to_string(),
    };
    // The rounded `human_size` alone can make a real, confirmed-different (`!same_bytes`) change
    // read as "unchanged" — a 2-byte-larger 81.3 KB file still prints "81.3 KB → 81.3 KB" (real bug:
    // the byte-for-byte comparison already *knows* it changed; the display just never said how).
    // Appended only when both sides' sizes are known — with one side `Absent`, there is nothing to
    // take a delta *of*, and the bare "(なし) → 22.3 KB" already says everything a delta could.
    let delta = match (old_len, new_len) {
        (Some(o), Some(n)) if o == n => {
            Some(tr(lang, crate::i18n::Msg::MediaDiffSameSizeDifferentContent).to_string())
        }
        (Some(o), Some(n)) => Some(format_byte_delta(lang, n as i64 - o as i64)),
        _ => None,
    };
    let mut line = format!(
        "{}: {} → {}",
        tr(lang, crate::i18n::Msg::MediaDiffBinaryFile),
        fmt(old_len),
        fmt(new_len)
    );
    if let Some(delta) = delta {
        line.push_str(&format!(" ({delta})"));
    }
    Some(line)
}

/// The signed byte delta for `media_summary_line`'s "(+2 B)"/"(-2 B)" suffix — thousands-separated
/// so a large delta (a multi-MB binary change) stays readable, e.g. "+1,234,567 B".
fn format_byte_delta(lang: crate::i18n::Lang, delta: i64) -> String {
    let sign = if delta >= 0 { '+' } else { '-' };
    format!(
        "{sign}{} {}",
        thousands_separated(delta.unsigned_abs()),
        tr(lang, crate::i18n::Msg::MediaDiffByteUnit)
    )
}

/// `1234567` → `"1,234,567"`.
fn thousands_separated(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// The centered body text for `render_gitdiff_source`'s empty-diff branch
/// (`docs/FEATURE-MEDIA-DIFF.md` §5) — the plain "(no changes)" for every ordinary text kind (an
/// empty line diff there always was genuine — `App::diff_binary_summary_eligible`'s own gate), and
/// for the narrower set of kinds that gate covers (video/archive/table/unsupported/an image-mode
/// command), consults the media-diff worker instead of trusting the empty line diff at face value:
/// a real binary change renders as the summary line, a landed "identical bytes" or a still-pending
/// result render as "(no changes)"/"computing…" respectively.
fn empty_diff_body_text(app: &mut App, inner: Rect) -> String {
    let lang = app.lang;
    let no_changes = tr(lang, crate::i18n::Msg::GitNoChanges).to_string();
    let Some(PreviewKind::GitDiff(path)) = app.tab.preview_kind.clone() else {
        return no_changes;
    };
    if !app.diff_binary_summary_eligible(&path) {
        return no_changes;
    }
    let raster_px = media_diff_raster_px(app, inner).unwrap_or((64, 64));
    match app.poll_media_diff(&path, 1, raster_px) {
        None => tr(lang, crate::i18n::Msg::DiffComputing).to_string(),
        Some(outcome) => media_summary_line(lang, &outcome).unwrap_or(no_changes),
    }
}

/// less-style windowed rendering for large Code/Text files.
/// Reads only the visible window (from the start byte to the screen height) and colors Code with syntect (does not read the whole file).
/// Vertical scrolling is done by the "window cutout position", so Paragraph's vertical scroll stays 0. Only horizontal scroll is used.
fn render_windowed(frame: &mut Frame, app: &mut App, area: Rect) {
    // The block's geometry doesn't depend on its title (with a top border, the title sits *on* that
    // border row), so measure the inner rect first: the title needs the scroll label, which needs
    // the viewport height. Same ordering trick `render_gitdiff` already uses for its width.
    let extent = app.window_scroll_extent(Block::bordered().inner(area).height);
    let mut title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());
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
    let mut block = Block::bordered().title(title);
    if let Some(e) = extent {
        block = block.title(scroll_title(app.lang, e));
    }
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
        // The extent is already known here (the window is open; only the coloring is still
        // compiling), so draw the bar too rather than have it pop in a frame later.
        if let Some(e) = extent {
            render_scrollbar(frame, area, e);
        }
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
    if let Some(e) = extent {
        render_scrollbar(frame, area, e);
    }
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

/// Scroll position indicator: the `Top`/`Bot`/`All`/`NN%` title label and the thumb drawn on the
/// block's right border column.
///
/// The drawing tests go through the real key path (`handle_key`) and the real full-UI draw
/// (`ui::render`), and then read the drawn buffer, because every acceptance criterion here is about
/// what ends up on screen — not about the arithmetic that got it there.
#[cfg(test)]
mod scroll_indicator_tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::i18n::Lang;
    use crate::test_support::unique_tmp;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(app: &mut App, code: KeyCode) {
        crate::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    }

    /// Write `body` as `doc.txt` in a fresh sandbox, open its full-screen preview through the real
    /// key path (Enter on the tree cursor), and draw once.
    fn preview_of(
        name: &str,
        body: &str,
        w: u16,
        h: u16,
    ) -> (App, Terminal<TestBackend>, crate::test_support::TmpDir) {
        let dir = unique_tmp(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("doc.txt"), body).unwrap();
        let root = dir.canonicalize().unwrap();
        let mut app = App::new(root, Config::default()).unwrap();
        let i = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.file_name().is_some_and(|n| n == "doc.txt"))
            .expect("doc.txt がツリーに出る");
        app.tab.selected = i;
        press(&mut app, KeyCode::Enter);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        draw(&mut term, &mut app);
        (app, term, dir)
    }

    /// The screen, one String per row.
    fn rows(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// `(track rows, thumb rows)` read off the preview block's right border column, between its two
    /// corners. Also asserts every cell there is either the border or the thumb — i.e. the
    /// indicator never eats the frame.
    #[track_caller]
    fn bar(term: &Terminal<TestBackend>) -> (Vec<u16>, Vec<u16>) {
        let buf = term.backend().buffer();
        let x = buf.area.width - 1;
        let at = |y: u16| buf[(x, y)].symbol().to_string();
        let top = (0..buf.area.height)
            .find(|&y| at(y) == "┐")
            .expect("枠の右上コーナー");
        let bot = (0..buf.area.height)
            .find(|&y| at(y) == "┘")
            .expect("枠の右下コーナー");
        let (mut track, mut thumb) = (Vec::new(), Vec::new());
        for y in (top + 1)..bot {
            let s = at(y);
            assert!(
                s == "│" || s == "█",
                "右端は枠か thumb のはず: {s:?} (row {y})"
            );
            track.push(y);
            if s == "█" {
                thumb.push(y);
            }
        }
        (track, thumb)
    }

    /// vim's `%P` semantics, in both languages: `All` when nothing is off-screen, `Top`/`Bot` at
    /// the two ends, and a percentage of the scrollable range in between (so the end really is
    /// 100 %, which the old `byte_top / file_len` could never reach).
    #[test]
    fn scroll_label_reports_all_top_bot_and_percent() {
        let all = ScrollExtent::new(0, 0, 20);
        let top = ScrollExtent::new(0, 200, 20);
        let mid = ScrollExtent::new(50, 200, 20);
        let bot = ScrollExtent::new(200, 200, 20);

        assert_eq!(scroll_label(Lang::En, all), "All");
        assert_eq!(scroll_label(Lang::En, top), "Top");
        assert_eq!(scroll_label(Lang::En, mid), "25%");
        assert_eq!(scroll_label(Lang::En, bot), "Bot");

        assert_eq!(scroll_label(Lang::Jp, all), "全体");
        assert_eq!(scroll_label(Lang::Jp, top), "先頭");
        assert_eq!(scroll_label(Lang::Jp, mid), "25%");
        assert_eq!(scroll_label(Lang::Jp, bot), "末尾");

        // A position past the end (a resize not yet reconciled) still reads as the end, not as
        // some percentage above 100.
        assert_eq!(
            scroll_label(Lang::En, ScrollExtent::new(999, 200, 20)),
            "Bot"
        );

        // Drawn as its own right-aligned title, so a long path can never push it off the border.
        let title = scroll_title(Lang::En, bot);
        assert_eq!(title.to_string(), " [Bot] ");
        assert_eq!(title.alignment, Some(ratatui::layout::Alignment::Right));
    }

    /// The marker survives a path long enough to fill the whole top border — the case that made a
    /// plain title suffix unusable (the position is the one thing that must not be the first to be
    /// clipped).
    #[test]
    fn a_long_path_does_not_push_the_marker_off_the_border() {
        let body: String = (1..=400).map(|i| format!("line {i}\n")).collect();
        let (_, term, dir) = preview_of("konoma_ui_scroll_longpath", &body, 30, 14);
        let top = &rows(&term)[0..3].join("\n");
        assert!(
            top.contains("[Top]"),
            "パスが枠幅を埋めても位置マーカーは残る:\n{top}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Acceptance criteria 1 and 3: at the start the label says `Top` and the thumb sits at the top
    /// of the track; at the end it says `Bot` and the thumb's bottom edge touches the track's last
    /// row.
    #[test]
    fn thumb_reaches_the_track_end_when_scrolled_to_the_bottom() {
        let body: String = (1..=400).map(|i| format!("line {i}\n")).collect();
        let (mut app, mut term, dir) = preview_of("konoma_ui_scroll_ends", &body, 40, 20);

        let (track, thumb) = bar(&term);
        assert!(!thumb.is_empty(), "スクロールが要る文書では thumb が出る");
        assert_eq!(
            thumb.first(),
            track.first(),
            "先頭では thumb がトラック上端から始まる"
        );
        assert_ne!(
            thumb.last(),
            track.last(),
            "先頭では thumb が下端に届いていない(残量が見える)"
        );
        assert!(
            rows(&term).iter().any(|r| r.contains("[Top]")),
            "先頭のラベルは Top:\n{}",
            rows(&term).join("\n")
        );

        press(&mut app, KeyCode::Char('G'));
        draw(&mut term, &mut app);
        let (track, thumb) = bar(&term);
        assert_eq!(
            thumb.last(),
            track.last(),
            "末尾では thumb がトラック下端に接する"
        );
        assert!(
            rows(&term).iter().any(|r| r.contains("[Bot]")),
            "末尾のラベルは Bot:\n{}",
            rows(&term).join("\n")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Acceptance criterion 2: content that fits entirely gets a full-length thumb and the label `All`.
    #[test]
    fn thumb_fills_the_track_when_everything_fits() {
        let (_, term, dir) = preview_of("konoma_ui_scroll_all", "alpha\nbeta\ngamma\n", 40, 20);
        let (track, thumb) = bar(&term);
        assert_eq!(thumb, track, "全部見えているときは thumb がトラック全長");
        assert!(
            rows(&term).iter().any(|r| r.contains("[All]")),
            "収まっているときのラベルは All:\n{}",
            rows(&term).join("\n")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Acceptance criterion 4: the thumb is a visual measure of how much is left — the longer the
    /// file, the shorter it gets.
    #[test]
    fn thumb_shrinks_as_the_file_grows() {
        let short: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let long: String = (1..=4000).map(|i| format!("line {i}\n")).collect();
        let (_, short_term, short_dir) = preview_of("konoma_ui_scroll_short", &short, 40, 20);
        let (_, long_term, long_dir) = preview_of("konoma_ui_scroll_long", &long, 40, 20);

        let (track, short_thumb) = bar(&short_term);
        let (_, long_thumb) = bar(&long_term);
        assert!(
            long_thumb.len() < short_thumb.len(),
            "長いファイルほど thumb が短い: 長={} 短={}",
            long_thumb.len(),
            short_thumb.len()
        );
        assert!(
            short_thumb.len() < track.len(),
            "スクロールが要る文書では thumb は全長にならない"
        );
        assert!(!long_thumb.is_empty(), "極端に長くても thumb は消えない");
        std::fs::remove_dir_all(&short_dir).ok();
        std::fs::remove_dir_all(&long_dir).ok();
    }

    /// Acceptance criterion 6: the indicator is drawn *on* the border column, so the text area is
    /// exactly as wide as it was before. A line that is exactly as wide as the text area must still
    /// occupy a single row — had a column been taken, it would wrap and push the next line down.
    #[test]
    fn the_indicator_does_not_steal_a_text_column() {
        let w = 40u16;
        let text_w = (w - 2) as usize; // the block's two border columns
        let body = format!("{}\nSECOND\n", "x".repeat(text_w));
        let (_, term, dir) = preview_of("konoma_ui_scroll_width", &body, w, 20);

        let screen = rows(&term);
        let second = screen
            .iter()
            .position(|r| r.contains("SECOND"))
            .expect("2行目が描かれる");
        let filled = &screen[second - 1];
        assert_eq!(
            filled.matches('x').count(),
            text_w,
            "枠内いっぱい({text_w}桁)の行が折り返さず1行に収まる:\n{}",
            screen.join("\n")
        );
        let cells: Vec<char> = filled.chars().collect();
        assert_eq!(cells[0], '│', "左端は枠");
        assert!(
            cells[w as usize - 1] == '│' || cells[w as usize - 1] == '█',
            "右端は枠かインジケータ"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The label is not decoration bolted onto the windowed reader alone: the decorated Markdown
    /// view (which scrolls in wrapped display rows, not bytes) reports position the same way, and
    /// used to report nothing at all.
    #[test]
    fn decorated_markdown_reports_its_position_too() {
        let dir = unique_tmp("konoma_ui_scroll_md");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (1..=200).map(|i| format!("- item {i}\n")).collect();
        std::fs::write(dir.join("doc.md"), &body).unwrap();
        let root = dir.canonicalize().unwrap();
        let mut app = App::new(root, Config::default()).unwrap();
        let i = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.file_name().is_some_and(|n| n == "doc.md"))
            .expect("doc.md がツリーに出る");
        app.tab.selected = i;
        press(&mut app, KeyCode::Enter);
        let mut term = Terminal::new(TestBackend::new(40, 20)).unwrap();
        draw(&mut term, &mut app);
        assert!(!app.is_windowed(), "装飾 Markdown は windowed ではない");

        let (track, thumb) = bar(&term);
        assert_eq!(thumb.first(), track.first(), "先頭では thumb が上端");
        assert!(rows(&term).iter().any(|r| r.contains("[Top]")));

        press(&mut app, KeyCode::Char('G'));
        draw(&mut term, &mut app);
        let (track, thumb) = bar(&term);
        assert_eq!(thumb.last(), track.last(), "末尾では thumb が下端に接する");
        assert!(
            rows(&term).iter().any(|r| r.contains("[Bot]")),
            "末尾のラベルは Bot:\n{}",
            rows(&term).join("\n")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod help_tests {
    use super::help_sections;
    use crate::app::App;
    use crate::config::Config;
    use crate::test_support::unique_tmp;

    /// The decorated Markdown view's `?` help must not advertise `v / V → y` range selection (it's
    /// a no-op there — `preview_enter_visual` requires `is_windowed()`), but keeps `Y` with a text
    /// specific to that view (`AtRefPathHelp`: it only ever copies the whole-file `@path`, since
    /// there's no caret/selection concept on a Tab-focused item). The raw source view (`R`) is
    /// windowed, so it keeps both the original `v / V → y` row and the caret/selection-aware `Y`
    /// text (`AtRefHelp`).
    #[test]
    fn decorated_markdown_help_drops_v_row_raw_source_keeps_it() {
        let dir = unique_tmp("konoma_md_help_vrow_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), b"# t\n\nbody\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
        app.tree_activate().unwrap();

        // Decorated view (default): no `v / V → y` row; `Y` uses the path-only text.
        assert!(!app.is_windowed(), "前提: 装飾ビューは windowed でない");
        let sections = help_sections(&app);
        let rows = &sections[0].rows;
        assert!(
            !rows.iter().any(|(k, _)| k == "v / V → y"),
            "装飾ビューに v/V 選択行が出ている: {rows:?}"
        );
        let y_row = rows
            .iter()
            .find(|(k, _)| k == "Y")
            .expect("Y 行が無い")
            .1
            .clone();
        assert_eq!(
            y_row,
            crate::i18n::tr(app.lang, crate::i18n::Msg::AtRefPathHelp),
            "装飾ビューの Y は @path 専用の文言"
        );

        // Raw source view (`R`): windowed, so v/V → y comes back, and Y reverts to the
        // caret/selection-aware text.
        app.toggle_md_raw();
        assert!(app.is_windowed(), "前提: R 後は windowed");
        let sections = help_sections(&app);
        let rows = &sections[0].rows;
        assert!(
            rows.iter().any(|(k, _)| k == "v / V → y"),
            "ソース表示に v/V 選択行が無い: {rows:?}"
        );
        let y_row = rows
            .iter()
            .find(|(k, _)| k == "Y")
            .expect("Y 行が無い")
            .1
            .clone();
        assert_eq!(
            y_row,
            crate::i18n::tr(app.lang, crate::i18n::Msg::AtRefHelp),
            "ソース表示の Y はキャレット/選択対応の文言"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(all(test, feature = "git"))]
mod gitdiff_tests {
    use super::help_sections;
    use crate::app::App;
    use crate::config::Config;
    use crate::test_support::unique_tmp;
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

    /// The git diff is the third scrolling preview, so it reports position like the other two: a
    /// diff taller than the screen starts at `Top` with the thumb at the top of the track, and `G`
    /// takes it to `Bot` with the thumb's bottom edge on the track's last row.
    #[test]
    fn gitdiff_preview_reports_scroll_position() {
        let dir = unique_tmp("konoma_ui_gitdiff_scroll");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.rs");
        std::fs::write(&f, b"seed\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "init"]);
        // Enough added lines that the diff cannot fit in the viewport below.
        let body: String = (1..=200).map(|i| format!("added {i}\n")).collect();
        std::fs::write(&f, body).unwrap();

        let canon = dir.canonicalize().unwrap();
        let mut app = App::new(canon.clone(), Config::default()).unwrap();
        app.open_git_diff(&canon.join("a.rs"));
        let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();

        // The block's right border column, between its corners: `│` is track, `█` is thumb.
        let bar = |term: &Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            let x = buf.area.width - 1;
            let (mut track, mut thumb) = (Vec::new(), Vec::new());
            for y in 1..(buf.area.height - 1) {
                let s = buf[(x, y)].symbol().to_string();
                assert!(s == "│" || s == "█", "右端は枠か thumb: {s:?} (row {y})");
                track.push(y);
                if s == "█" {
                    thumb.push(y);
                }
            }
            (track, thumb)
        };
        let screen = |term: &Terminal<TestBackend>| -> String {
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };

        term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
            .unwrap();
        let (track, thumb) = bar(&term);
        assert_eq!(thumb.first(), track.first(), "先頭では thumb が上端");
        assert_ne!(thumb.last(), track.last(), "先頭では下端に届かない");
        assert!(screen(&term).contains("[Top]"), "先頭のラベルは Top");

        app.preview_to_bottom();
        term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
            .unwrap();
        let (track, thumb) = bar(&term);
        assert_eq!(
            thumb.last(),
            track.last(),
            "末尾では thumb がトラック下端に接する"
        );
        assert!(screen(&term).contains("[Bot]"), "末尾のラベルは Bot");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Render the GitDiff preview of a modified file and verify (1) changed rows have red/green background cells,
    /// (2) both the deleted line content (beta) and the added line content (gamma) appear on screen.
    #[test]
    fn gitdiff_preview_renders_with_colored_rows() {
        let dir = unique_tmp("konoma_ui_gitdiff_render");
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
        let dir = unique_tmp("konoma_ui_gitdiff_clean");
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

    /// The `?` help's `R` row in `Surface::PreviewGitDiff` must be gated by the same
    /// `diff_representation_count` predicate the footer's own hint already uses
    /// ([[hint-shown-iff-key-acts]]): omitted entirely for a target with just one representation
    /// (an unsupported extension here), worded "source ⇄ preview" for a windowed-capable text kind
    /// (two representations), and the full "source → rendered → preview" for Markdown (three).
    /// Previously this row was unconditional and always read the three-way text, so it lied about
    /// `R` for the first two cases.
    #[test]
    fn gitdiff_help_r_row_matches_representation_count() {
        let dir = unique_tmp("konoma_ui_gitdiff_help_r_row");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        // A NUL byte forces `is_probably_text` to classify this as binary (`resolve_preview`
        // degrades to `CanNotPreview`) — an extension with no matching rule alone still resolves to
        // ordinary `Text` for text content (two representations), which is not what this row wants
        // to exercise.
        std::fs::write(dir.join("a.xyz"), b"seed\0\n").unwrap();
        std::fs::write(dir.join("a.rs"), b"seed\n").unwrap();
        std::fs::write(dir.join("a.md"), b"seed\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "init"]);
        std::fs::write(dir.join("a.xyz"), b"changed\0\n").unwrap();
        std::fs::write(dir.join("a.rs"), b"changed\n").unwrap();
        std::fs::write(dir.join("a.md"), b"changed\n").unwrap();

        let canon = dir.canonicalize().unwrap();
        let mut app = App::new(canon.clone(), Config::default()).unwrap();

        // One representation (CanNotPreview): no `R` row at all.
        app.open_git_diff(&canon.join("a.xyz"));
        assert!(
            matches!(
                app.cfg.resolve_preview(&canon.join("a.xyz")),
                crate::preview::PreviewKind::CanNotPreview { .. }
            ),
            "前提: a.xyz は CanNotPreview に解決されるはず"
        );
        let sections = help_sections(&app);
        assert!(
            !sections[0].rows.iter().any(|(k, _)| k == "R"),
            "1 表現なのに R 行が出ている: {:?}",
            sections[0].rows
        );

        // Two representations (code): "source ⇄ preview".
        app.open_git_diff(&canon.join("a.rs"));
        let sections = help_sections(&app);
        let r_row = sections[0]
            .rows
            .iter()
            .find(|(k, _)| k == "R")
            .expect("2 表現なら R 行がある")
            .1
            .clone();
        assert_eq!(
            r_row,
            crate::i18n::tr(app.lang, crate::i18n::Msg::DiffViewCycleHelpPair),
            "2 表現の R 行は source ⇄ preview のはず"
        );

        // Three representations (Markdown): the full cycle text.
        app.open_git_diff(&canon.join("a.md"));
        let sections = help_sections(&app);
        let r_row = sections[0]
            .rows
            .iter()
            .find(|(k, _)| k == "R")
            .expect("3 表現なら R 行がある")
            .1
            .clone();
        assert_eq!(
            r_row,
            crate::i18n::tr(app.lang, crate::i18n::Msg::DiffViewCycleHelp),
            "3 表現の R 行は source → rendered → preview のはず"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
