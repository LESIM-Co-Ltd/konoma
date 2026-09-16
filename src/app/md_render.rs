use super::*;

/// Which document(s) `App::build_decorated` renders from — the seam that lets the diff's
/// `Rendered` presentation (`docs/FEATURE-MD-RENDERED-DIFF.md` §2) share the **entire** decorated-
/// Markdown pipeline (image/mermaid/math resolution included) with an ordinary preview, instead of
/// a parallel cache/render path of its own with its own, deliberately degraded image handling.
enum DecoratedSource<'a> {
    /// The file's current on-disk content at `path` (the ordinary, non-diff preview).
    File,
    /// Both versions' text, already through [`App::preprocess_md_src`] (front matter stripped,
    /// footnotes/inline HTML already rewritten) — never read from disk inside `build_decorated`
    /// itself. `App::ensure_md_cache` is the one caller that produces this, from
    /// `App::diff_rendered_sources`.
    Diff { old_pre: &'a str, new_pre: &'a str },
}

impl App {
    /// Build (or reuse) the decorated cache for a Markdown/Mermaid/code preview, **or** the diff's
    /// `Rendered` presentation, at display width `width`. Which of the two `self.tab.preview_kind`/
    /// `self.tab.diff_view` currently call for is `MdCacheSource` — part of the cache key alongside
    /// `path`/`width` (see that type's own doc comment for why: `R` cycling within
    /// `Surface::PreviewGitDiff` changes `diff_view` for the *same* `path`/`width` without going
    /// through `App::open_git_diff` again). On a miss this renders, collapses links, collects the
    /// Tab items (skipped for the `Rendered` presentation — it has none), and precomputes the wrap
    /// layout (row prefix sums via ratatui's own per-line reflow) — all the per-document work, done
    /// once. Frames then only slice the visible range (`md_slice`).
    pub(super) fn ensure_md_cache(&mut self, width: u16) {
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let want_diff_rendered = matches!(self.tab.preview_kind, Some(PreviewKind::GitDiff(_)))
            && self.tab.diff_view == DiffView::Rendered;
        let want_source = if want_diff_rendered {
            MdCacheSource::Diff
        } else {
            MdCacheSource::File
        };
        // If the fence diagram's target row count (fit-to-view) changed, only rebuild documents
        // that contain a diagram (a viewport height change does not rebuild every md every time).
        let fence_rows = self.mermaid_fit_rows();
        if matches!(&self.md_cache, Some(c) if c.path == path && c.width == width && c.source == want_source
            && (c.fence_rows == fence_rows
                || !c.images.iter().any(|p| crate::preview::markdown::is_mermaid_fence_url(&p.url))))
        {
            return;
        }

        // The `Rendered` presentation needs both versions' preprocessed text up front — resolved
        // here (not inside `build_decorated`, which never touches disk/git itself) so a failure
        // (unreadable, non-UTF-8, over the size cap) can fall back to `Source` with a flash before
        // any rendering is attempted. `owned_diff_src` outlives the `DecoratedSource` borrowing it.
        let owned_diff_src: Option<(String, String)> = if want_diff_rendered {
            match self.diff_rendered_sources(&path) {
                Some(pair) => Some(pair),
                None => {
                    self.tab.diff_view = DiffView::Source;
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedUnavailable).into());
                    return;
                }
            }
        } else {
            None
        };
        let cache_source = if owned_diff_src.is_some() {
            MdCacheSource::Diff
        } else {
            MdCacheSource::File
        };
        let source = match &owned_diff_src {
            Some((old_pre, new_pre)) => DecoratedSource::Diff { old_pre, new_pre },
            None => DecoratedSource::File,
        };

        // Whether a change-gutter column will end up non-empty — decided **before** any
        // width-dependent rendering (`App::gutter_will_be_active`'s own doc comment): a real bug
        // (confirmed on a real terminal) was rendering the body at the full `width` and only
        // prepending the 1-cell gutter afterwards, which let a full-width line (a heading's own
        // underline rule, a centered mermaid/image placeholder row) overflow by exactly 1 column —
        // wrapping into a spurious extra row, and desynchronizing `row_prefix`/`ImagePlacement.line`
        // from what `md_slice` actually draws (a diagram ending up drawn *over* the heading-rule row
        // below it). Rendering 1 column narrower up front keeps the gutter+body total at exactly
        // `width`, matching what the previous — no-gutter — layout already fit into.
        let gutter_active = self.gutter_will_be_active(&path, &source);
        let render_width = if gutter_active {
            width.saturating_sub(1)
        } else {
            width
        };

        let decorated = self.build_decorated(&path, render_width, &source);
        // Kick off background downloads for any remote images shown as "loading". Each completes
        // by invalidating md_cache (apply_remote_fetch) so this rebuilds with the cached file.
        // A *synchronous* failure (remote images disabled: `ensure_remote_md_fetch` returns true
        // the first time it marks the URL failed, since nothing else would ever invalidate the
        // cache to un-stick the Loading placeholder) also asks for a resync below, the same way a
        // synchronous mermaid/math completion does.
        let mut resync = false;
        for url in &decorated.remote_urls {
            resync |= self.ensure_remote_md_fetch(url);
        }
        // Kick off background renders for any ```mermaid fences not yet in the diagram cache
        // (content-hash keyed). Completion invalidates md_cache (apply_md_image), so the loading
        // line re-lays out into the real inline diagram — the remote-image pattern exactly. Applies
        // identically to the `Rendered` presentation now: `decorated.mermaid_fences` was collected
        // from *both* versions' text (`build_decorated`'s own `DecoratedSource::Diff` arm), so a
        // diagram that only exists in the old version still gets an encode request here.
        // Synchronous completions (no loader tx = tests) land *before* we store the cache below,
        // so the just-built decoration is already stale — rebuild once with the results in.
        for code in &decorated.mermaid_fences {
            if code.trim().is_empty() {
                continue; // an empty fence (mid-typing) never becomes a diagram = don't kick off rendering
            }
            resync |= self.ensure_mermaid_fence_render(code.clone());
        }
        // Math expressions follow the same pattern: kick off rendering for any not-yet-arrived
        // latex+display key (the same path as mermaid fences).
        for (latex, display) in &decorated.math_exprs {
            if latex.trim().is_empty() {
                continue;
            }
            resync |= self.ensure_math_render(latex.clone(), *display);
        }
        // Reclaim fence/math keys that **no longer exist** in the current document. Since the key
        // is a content hash, an external edit (agent-watch's repeated edits) generates a new key
        // every time the content changes, and the old key's decoded raster/protocol/SVG grow
        // monotonically until the file is switched, unreferenced by anything.
        {
            let mut live: std::collections::HashSet<PathBuf> = decorated
                .mermaid_fences
                .iter()
                .map(|c| PathBuf::from(crate::preview::markdown::mermaid_fence_url(c)))
                .collect();
            live.extend(
                decorated
                    .math_exprs
                    .iter()
                    .map(|(l, d)| PathBuf::from(crate::preview::markdown::math_url(l, *d))),
            );
            self.md_image_cache.retain(|k, _| {
                let s = k.to_string_lossy();
                !(crate::preview::markdown::is_mermaid_fence_url(&s)
                    || crate::preview::markdown::is_math_url(&s))
                    || live.contains(k)
            });
        }
        let mut decorated = if resync {
            self.build_decorated(&path, render_width, &source)
        } else {
            decorated
        };
        let (lines, targets) = self.postprocess_md(decorated.lines);
        // Re-resolve every inline math placement's own `col` against these **decorated** lines — see
        // `render_inline_math`'s own doc comment ("`col` recorded here is *provisional*") for the full
        // reasoning and the real, confirmed bug this closes (a link/checkbox/emoji/autolink earlier on
        // the same line changing width during `postprocess_md` left the math visibly offset from its
        // own reserved cells). Must run after `postprocess_md`, before `lines`/`decorated.images` are
        // stored into the cache below.
        Self::resolve_inline_math_cols(&lines, &mut decorated.images);
        // The `Rendered` presentation has no Tab-focus interactivity at all (`docs/
        // FEATURE-MD-RENDERED-DIFF.md` §3: "items は Diff では空") — reading/toggling a checkbox or
        // opening a link would have to pick one of the two versions to write back to, which this
        // view has no sensible answer for. Everything else about the render (images, mermaid,
        // math, block coloring) is otherwise identical to an ordinary preview.
        let items = match cache_source {
            MdCacheSource::Diff => Vec::new(),
            MdCacheSource::File => {
                build_md_items_from_render(&lines, &targets, &decorated.images, &decorated.extras)
            }
        };
        let anchors = compute_md_anchors(&lines);
        // `lines` was rendered at `render_width` (1 narrower than `width` when `gutter_active` —
        // see this function's own doc comment above). `max_line_cols` measures those lines as
        // rendered, so it must gain the same 1 column back once the gutter is actually prepended
        // (`md_slice`'s own `with_diff_gutter`/`with_preview_diff_gutter`), or the horizontal-scroll
        // ceiling (`max_line_cols.saturating_sub(inner.width)`) would under-count by exactly the
        // gutter's own width.
        let max_line_cols =
            lines.iter().map(|l| l.width()).max().unwrap_or(0) + usize::from(gutter_active);
        // Wrapped at `render_width`, matching what `lines` was actually rendered at — equivalent to
        // wrapping the gutter-prepended line at the full `width` (a single leading, non-breaking
        // 1-cell glyph shifts every wrap point by exactly 1 column either way), which is what
        // `md_slice`'s per-frame `Paragraph` (given the *real*, gutter-prepended line) wraps at.
        let row_prefix = if self.cfg.ui.wrap && render_width > 0 {
            use ratatui::text::Text;
            use ratatui::widgets::{Paragraph, Wrap};
            let mut pre = Vec::with_capacity(lines.len() + 1);
            let mut acc = 0usize;
            pre.push(0);
            for line in &lines {
                acc += Paragraph::new(Text::from(vec![line.clone()]))
                    .wrap(Wrap { trim: false })
                    .line_count(render_width)
                    .max(1);
                pre.push(acc);
            }
            pre
        } else {
            Vec::new()
        };
        // Mirror the items into the live field (the same refresh decorate_md_items used to do per
        // frame — items only change when the cache is rebuilt) and keep the focus index in range.
        self.md_items = items.clone();
        match self.tab.focused_item {
            Some(_) if self.md_items.is_empty() => self.tab.focused_item = None,
            Some(f) if f >= self.md_items.len() => {
                self.tab.focused_item = Some(self.md_items.len() - 1)
            }
            _ => {}
        }
        // The two kinds of change gutter this cache can carry — never both at once (one `MdCache`
        // is either an ordinary preview or a `Rendered` diff, never both). `preview_gutter_marks`
        // (§3) is the ordinary decorated Markdown preview's own gutter against its committed state,
        // computed here from `decorated.pre_src`/`extras.block_rows`; `diff_marks` (§2) is simply
        // `decorated.diff_marks` — `App::build_decorated`'s `DecoratedSource::Diff` arm already
        // computed it via `render_markdown_diff_aligned`.
        let (diff_marks, preview_gutter_marks) = match cache_source {
            MdCacheSource::Diff => {
                // §5's "front matter だけの変更" degeneration: the block-diff found nothing (front
                // matter is stripped before either side ever reaches `Doc::parse`), yet the file
                // *does* have a diff (the raw unified diff is non-empty) — explain why nothing is
                // marked rather than leaving the reader to wonder whether the feature is broken.
                if decorated.diff_marks.is_empty() && !self.git_diff_lines().is_empty() {
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedFrontMatterOnly).into());
                }
                (std::mem::take(&mut decorated.diff_marks), Vec::new())
            }
            MdCacheSource::File => {
                let pgm = if self.cfg.ui.git_gutter
                    && matches!(self.tab.preview_kind, Some(PreviewKind::Markdown(_)))
                {
                    self.preview_diff_marks(&decorated.pre_src, &decorated.extras.block_rows)
                } else {
                    Vec::new()
                };
                (Vec::new(), pgm)
            }
        };
        self.md_cache = Some(MdCache {
            path,
            width,
            lines,
            items,
            images: decorated.images,
            src_lines: decorated.src_lines,
            max_line_cols,
            row_prefix,
            fence_rows,
            anchors,
            details_states: crate::preview::markdown::current_details_states(),
            pre_src: decorated.pre_src,
            pre_origin: decorated.pre_origin,
            preview_gutter_marks,
            diff_marks,
            source: cache_source,
        });
    }

    /// Whether the change gutter (`docs/FEATURE-MD-RENDERED-DIFF.md` §2/§3) will end up non-empty
    /// for `source` — decided without rendering anything (`ensure_md_cache`'s own doc comment on
    /// why this has to happen *before* the width-dependent render, not after). `Diff` reuses
    /// `old_pre`/`new_pre` verbatim (`App::diff_rendered_sources` already resolved them);
    /// `MdCacheSource::File`'s own "no baseline = no gutter" and "`[ui] git_gutter`/Markdown-only"
    /// rules are mirrored here exactly (`App::preview_diff_marks`'s own doc comment), reading the
    /// file and its baseline a **second** time — the render itself reads them again inside
    /// `build_decorated_file`/`preview_diff_marks` — a small, bounded duplication (plain text
    /// reads + a `Doc::parse` diff, not a real render) in exchange for knowing the answer before
    /// any width-dependent work starts.
    fn gutter_will_be_active(&self, path: &Path, source: &DecoratedSource<'_>) -> bool {
        match source {
            DecoratedSource::Diff { old_pre, new_pre } => {
                crate::preview::markdown::diff_has_any_change(old_pre, new_pre)
            }
            DecoratedSource::File => {
                if !self.cfg.ui.git_gutter
                    || !matches!(self.tab.preview_kind, Some(PreviewKind::Markdown(_)))
                {
                    return false;
                }
                let Some(old_raw) = self.preview_diff_baseline(path) else {
                    return false;
                };
                let Ok(content) = crate::preview::text::load(path) else {
                    return false;
                };
                let new_raw = content.lines.join("\n");
                let old_pre = self.preprocess_md_src(&old_raw);
                let new_pre = self.preprocess_md_src(&new_raw);
                crate::preview::markdown::diff_has_any_change(&old_pre, &new_pre)
            }
        }
    }

    /// `old`/`new` text for the diff's `Rendered` presentation, already through the identical
    /// pre-pass chain the renderer itself parses (`App::preprocess_md_src`). `new` is the file's
    /// current on-disk bytes; `old` is the committed baseline the diff's `Source` presentation
    /// already compares against — the follow-session snapshot while `diff_follow_scope` is active
    /// and not toggled to the full range (`App::follow_baseline_contents`, the same baseline
    /// `App::compute_gitdiff_lines` selects), the backend's committed blob otherwise
    /// (`crate::vcs::base_contents`). No committed baseline at all (`None` — an untracked file, or
    /// one created since follow-start) reads as an empty string, matching §5's "旧版が無い…全ブロック
    /// Insert" rule — the *same* "missing = empty" contract `follow_baseline_diff` already applies
    /// to the unified diff. `None` overall only for a genuine read/size/encoding failure on either
    /// side (`ensure_md_cache`'s own caller then falls back to `Source`).
    ///
    /// `follow_baseline_contents` only exists on a `git`-feature build; on a no-`git` build,
    /// `diff_follow_scope` can never actually be `true` in practice (`follow_jump`'s own diff-
    /// opening branch only sets it after a non-empty `compute_gitdiff_lines`, which is always empty
    /// there — `crate::git::file_diff`'s no-`git` stub), so falling straight through to
    /// `crate::vcs::base_contents` there is not a behavior change, only what lets this function
    /// compile without the feature at all.
    fn diff_rendered_sources(&self, path: &Path) -> Option<(String, String)> {
        let new_bytes = std::fs::read(path).ok()?;
        if new_bytes.len() > FOLLOW_BASELINE_FILE_CAP {
            return None;
        }
        let new_raw = String::from_utf8(new_bytes).ok()?;

        #[cfg(feature = "git")]
        let old_bytes = if self.diff_follow_scope && !self.follow_diff_full {
            self.follow_baseline_contents(path)
        } else {
            crate::vcs::base_contents(&self.tab.root, path)
        };
        #[cfg(not(feature = "git"))]
        let old_bytes = crate::vcs::base_contents(&self.tab.root, path);

        let old_bytes = match old_bytes {
            Some(b) if b.len() > FOLLOW_BASELINE_FILE_CAP => return None,
            Some(b) => b,
            None => Vec::new(),
        };
        let old_raw = String::from_utf8(old_bytes).ok()?;

        Some((
            self.preprocess_md_src(&old_raw),
            self.preprocess_md_src(&new_raw),
        ))
    }

    /// The committed baseline for `App::preview_diff_marks`'s own `old_src`: `None` outside a
    /// repository, for an untracked/newly-added file (nothing committed to compare against — the
    /// caller treats that the same as "no gutter" rather than drawing every block `Added`, matching
    /// the code/text gutter's own "no marks at all outside a repo" contract, not the *diff*
    /// presentations' "empty baseline = all-added" one: this is an ordinary content preview, not a
    /// diff view, so an untracked file simply has no gutter to show, the same as it has none today),
    /// or when the bytes aren't valid UTF-8 (binary/mixed encoding — never attempted as Markdown).
    fn preview_diff_baseline(&self, path: &Path) -> Option<String> {
        let bytes = crate::vcs::base_contents(&self.tab.root, path)?;
        String::from_utf8(bytes).ok()
    }

    /// `docs/FEATURE-MD-RENDERED-DIFF.md` §3's own gutter marks for the **current** decorated
    /// Markdown preview: `new_pre_src`/`new_block_rows` are this cache build's own `pre_src`/
    /// `extras.block_rows` (the exact text/row-layout the renderer just produced), and the baseline
    /// is the committed version of the *same* path, put through the identical pre-pass chain
    /// (`preprocess_md_src`) so both sides are compared as the renderer would actually parse them —
    /// front matter stripped, footnotes/inline HTML already rewritten, matching
    /// `preview::markdown::markdown_preview_marks`'s own doc comment. Empty when there is no
    /// baseline to compare (`preview_diff_baseline`) or the file has genuinely not changed.
    fn preview_diff_marks(
        &self,
        new_pre_src: &str,
        new_block_rows: &[std::ops::Range<usize>],
    ) -> Vec<(
        std::ops::Range<usize>,
        crate::preview::markdown::PreviewMark,
    )> {
        let Some(path) = self.tab.preview_path.clone() else {
            return Vec::new();
        };
        let Some(old_raw) = self.preview_diff_baseline(&path) else {
            return Vec::new();
        };
        let old_pre_src = self.preprocess_md_src(&old_raw);
        crate::preview::markdown::markdown_preview_marks(&old_pre_src, new_pre_src, new_block_rows)
    }

    /// The exact pre-pass chain `build_decorated` applies to a Markdown file's raw bytes before
    /// handing the result to the renderer — front matter strip, then footnotes, then inline HTML,
    /// each gated by its own `[ui] md_*` setting — factored out so `preview_diff_marks`/
    /// `diff_rendered_sources` can put an arbitrary source string (never read from disk by this
    /// function, so it has no `LineOrigin` of its own to thread through) through the identical
    /// chain without a second, hand-copied mirror of it. The `LineOrigin` `build_decorated` itself
    /// keeps for the *current* file (for the checkbox-toggle write-back path) is discarded here — a
    /// diff comparison only needs the resulting text, never a line's own provenance.
    fn preprocess_md_src(&self, src: &str) -> String {
        let src = if self.cfg.ui.md_frontmatter {
            crate::preview::markdown::strip_front_matter(src).1
        } else {
            src.to_string()
        };
        let origin = crate::preview::markdown::identity_origin(&src);
        let (src, origin) = if self.cfg.ui.md_footnotes {
            crate::preview::markdown::process_footnotes_traced(&src, &origin)
        } else {
            (src, origin)
        };
        let (src, _origin) = if self.cfg.ui.md_inline_html {
            crate::preview::markdown::process_inline_html_traced(&src, &origin)
        } else {
            (src, origin)
        };
        src
    }

    /// The current decorated Markdown cache's first change-gutter mark, as a **visual** (post-wrap)
    /// display row — where `ui/preview.rs`'s shared decorated-preview body scrolls to on the first
    /// draw after `App::take_diff_scroll_pending` returns `true` (either a follow jump into a
    /// decorated Markdown document, §3, or a fresh `Rendered` diff open/cycle, §2 — whichever kind
    /// of mark the current cache actually carries, `MdCache::source` says which). Must be called
    /// after `md_layout`/`ensure_md_cache` has already built the cache for the current width
    /// (`md_visual_span` reads its wrap-row prefix sums); `None` when there is no cache yet or no
    /// mark at all.
    pub(crate) fn md_first_diff_mark_row(&self) -> Option<usize> {
        let c = self.md_cache.as_ref()?;
        let logical = match c.source {
            MdCacheSource::Diff => c.diff_marks.first()?.0.start,
            MdCacheSource::File => c.preview_gutter_marks.first()?.0.start,
        };
        Some(self.md_visual_span(logical).0)
    }

    /// Whether the current decorated Markdown cache carries a change-gutter column at all (either
    /// kind — `docs/FEATURE-MD-RENDERED-DIFF.md` §2's `diff_marks` or §3's `preview_gutter_marks`)
    /// — `ui/preview.rs::overlay_inline_images` reads this to shift every inline image one column
    /// right, matching the 1-cell gutter `App::md_slice` prepends to each line whenever either kind
    /// is non-empty (`with_diff_gutter`/`with_preview_diff_gutter`'s shared "no marks = no column"
    /// contract).
    pub(crate) fn md_gutter_active(&self) -> bool {
        self.md_cache
            .as_ref()
            .is_some_and(|c| !c.diff_marks.is_empty() || !c.preview_gutter_marks.is_empty())
    }

    /// The default open state of a `<details>` block from `ui.md_details` and its `open` attribute
    /// — [`details_default_open`], read off this `App`'s own config.
    fn details_default_open(&self, open_attr: bool) -> bool {
        details_default_open(&self.cfg.ui.md_details, open_attr)
    }

    /// `Space`/`Enter` on a focused `<details>` summary: flip its open state and rebuild the cache.
    pub fn toggle_details(&mut self, ordinal: usize) {
        let cur = self
            .md_cache
            .as_ref()
            .and_then(|c| c.details_states.get(ordinal).copied())
            .unwrap_or(true);
        self.details_open.insert(ordinal, !cur);
        self.md_cache = None; // reflect the open/closed change on the next rebuild
    }

    /// Scroll the current Markdown preview so the heading matching `slug` (a GitHub-style in-page
    /// anchor, minus the leading `#`) is at the top of the view. Returns false if there is no such
    /// heading (so the caller can flash). Keeps the decorated view (unlike a raw-source line jump).
    pub(super) fn md_scroll_to_anchor(&mut self, slug: &str) -> bool {
        let target = slug.trim().to_lowercase();
        let Some(logical) = self.md_cache.as_ref().and_then(|c| {
            c.anchors
                .iter()
                .find(|(s, _)| *s == target)
                .map(|(_, i)| *i)
        }) else {
            return false;
        };
        let (row, _) = self.md_visual_span(logical);
        // The draw path clamps preview_scroll against the document height, so an anchor near the
        // end simply scrolls as far as it can.
        self.tab.preview_scroll = row.min(u16::MAX as usize) as u16;
        true
    }

    /// Ensure the decorated cache for `width` and return (total display rows, widest line in cells).
    /// The total is wrap-aware via the cached prefix sums, so the scroll clamp matches what the
    /// renderer draws without re-laying-out the whole document every frame.
    pub fn md_layout(&mut self, width: u16) -> (usize, usize) {
        self.ensure_md_cache(width);
        let Some(c) = &self.md_cache else {
            return (0, 0);
        };
        let total = if self.cfg.ui.wrap {
            c.row_prefix.last().copied().unwrap_or(c.lines.len())
        } else {
            c.lines.len()
        };
        (total, c.max_line_cols)
    }

    /// The visible slice of the decorated lines for the (already clamped) display-row `scroll`,
    /// plus the residual scroll offset to pass to Paragraph (the first sliced line may start above
    /// the viewport when wrapped). Only on-screen lines are cloned and the focus inversion touches
    /// only the focused line — O(viewport) per frame instead of O(document).
    pub fn md_slice(&self, scroll: u16, height: u16) -> (Vec<Line<'static>>, u16) {
        let Some(c) = &self.md_cache else {
            return (Vec::new(), 0);
        };
        let s = scroll as usize;
        let h = height.max(1) as usize;
        let (lo, hi, local) = if self.cfg.ui.wrap && c.row_prefix.len() == c.lines.len() + 1 {
            // The logical-line range spanning the visible display rows [s, s+h). `prefix` is each
            // line's starting display row (monotonically increasing).
            let lo = c.row_prefix.partition_point(|&p| p <= s).saturating_sub(1);
            let hi = c
                .row_prefix
                .partition_point(|&p| p < s + h)
                .min(c.lines.len());
            (lo, hi.max(lo), (s - c.row_prefix[lo]) as u16)
        } else {
            let lo = s.min(c.lines.len());
            let hi = (s + h).min(c.lines.len());
            (lo, hi, 0)
        };
        let mut out: Vec<Line<'static>> = c.lines[lo..hi].to_vec();
        // While searching, emphasize matches on visible lines (the current match = orange / others
        // = yellow), using the same look and the same function as windowed reads. Since it only
        // touches the visible range, this is O(viewport) regardless of document size.
        if let Some(q) = self.tab.preview_search.as_deref() {
            let cur = self.tab.search_matches.get(self.tab.search_idx).copied();
            for (i, line) in out.iter_mut().enumerate() {
                let li = lo + i;
                // Skip this line if it has no match (avoids a clone/rebuild).
                if !self.tab.search_matches.iter().any(|(_, l, _)| *l == li) {
                    continue;
                }
                // If the current match is on this line, pass its within-line occurrence rank so only that one is orange.
                let rank = cur.and_then(|(_, cl, _)| {
                    (cl == li).then(|| {
                        self.tab.search_matches[..self.tab.search_idx]
                            .iter()
                            .filter(|(_, l, _)| *l == li)
                            .count()
                    })
                });
                *line = highlight_query_in_line(std::mem::take(line), q, rank);
            }
        }
        // Invert only the focused item's line (does nothing if it's off-screen = looks identical to inverting the whole document).
        if let Some(f) = self.tab.focused_item {
            if let Some(it) = c.items.get(f) {
                if it.line >= lo && it.line < hi {
                    // Only a code block inverts the whole line (its header band is already full-width).
                    // A fence diagram inverts only the caption span (inverting the whole line would
                    // also invert the centered indentation whitespace into a huge white bar — caught
                    // on a real Ghostty).
                    let whole = matches!(it.kind, MdItemKind::CodeBlock { .. });
                    // The marker's within-line ordinal = the count of items on the same line that come before it.
                    let first_on_line = c.items.partition_point(|x| x.line < it.line);
                    let ordinal = f - first_on_line;
                    out[it.line - lo] = invert_focused_line(&c.lines[it.line], ordinal, whole);
                }
            }
        }
        // The change gutter — the very last pass, after search highlight/focus inversion have
        // already touched the visible lines, matching `render_doc_diff`'s own "marks applied after
        // decoration" ordering. Exactly one of the two mark lists is ever non-empty for a given
        // cache (`MdCache::source`), so exactly one of these two calls ever actually does anything;
        // the other is a no-op via each helper's own "no marks = no column" early return.
        let out = with_diff_gutter(out, lo, &c.diff_marks);
        let out = with_preview_diff_gutter(out, lo, &c.preview_gutter_marks);
        (out, local)
    }

    /// Test-only view of the cached decorated lines (link-collapsed form, the same lines the
    /// renderer slices from). Production rendering goes through `md_layout` + `md_slice`.
    /// NOTE: the return value is already collapsed — feeding it into `decorate_md_items` runs
    /// `collapse_links` a second time, which finds no "(URL)" spans and leaves every rebuilt
    /// link item with an empty target. Fine for render/inversion assertions; do NOT use that
    /// combination to test link activation (use the cache built by `md_layout` instead).
    #[cfg(test)]
    pub fn decorated_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        self.ensure_md_cache(width);
        self.md_cache
            .as_ref()
            .map(|c| c.lines.clone())
            .unwrap_or_default()
    }

    /// Block-level inline images reserved in the current decorated Markdown (empty for other kinds
    /// or when there is no image backend). `decorated_lines` must be called first (it fills the cache).
    pub fn md_images(&self) -> Vec<crate::preview::markdown::ImagePlacement> {
        self.md_cache
            .as_ref()
            .map(|c| c.images.clone())
            .unwrap_or_default()
    }

    /// Read the file with a cap and generate decorated lines (plus inline-image placements and the list
    /// of remote image URLs to fetch) according to its kind. A read failure becomes a single safe line.
    /// Only Markdown yields image placements / remote URLs.
    ///
    /// `source` picks which document(s) to render (`DecoratedSource`'s own doc comment). The
    /// image/mermaid/math resolution setup below — `font`/`base_dir`/`avail` and the `slot_of`/
    /// `mermaid_slot`/`math_slot` closures — is built exactly **once**, shared verbatim by both
    /// branches: resolving a picture, a ```mermaid fence, or a LaTeX expression depends only on
    /// `path`'s own directory and content-hash-keyed caches (`self.md_image_cache`), never on which
    /// of the diff's two versions a particular block happens to come from — so the `Rendered`
    /// presentation draws real images/diagrams/equations exactly the way an ordinary preview does,
    /// not a text-only fallback of its own.
    fn build_decorated(
        &self,
        path: &Path,
        width: u16,
        source: &DecoratedSource<'_>,
    ) -> DecoratedMarkdown {
        let theme = &self.cfg.ui.theme;
        let code = crate::preview::markdown::CodeStyle {
            bg: theme.code_bg(),
            label_bg: theme.code_label_bg(),
            label_right: theme.code_label_right(),
            tab_width: self.cfg.ui.tab_width,
            wrap: self.cfg.ui.wrap,
        };
        // Decide how to render each image URL. A local file or a cached remote fetch resolves to
        // a path → Inline (with its display size in cells). An uncached remote URL is Loading
        // (a fetch is kicked off separately, in `ensure_md_cache`) unless it has already failed.
        // Anything else (no backend / missing file / data: URL) degrades to text (principle #3).
        let font = self.picker.as_ref().map(|p| p.font_size());
        let base_dir = path.parent().map(|p| p.to_path_buf());
        let avail = width.saturating_sub(2);
        // `max_cols`: the caller's own width budget for *this one* image, when it has a
        // narrower one than the page's. `None` — every standalone block image, i.e. every
        // caller that existed before table cells could hold real pixels — means "the whole
        // page", `avail`, exactly as before. `Some(w)` comes from a table cell
        // (`render_table_cells`), whose column is far narrower than the page: the very same
        // `md_image_cells` fit runs, just against `w` instead of `avail`, so a cell image is
        // sized by the one sizing rule in this file rather than by a second, cell-only one.
        let slot_of = |url: &str, max_cols: Option<u16>| -> crate::preview::markdown::ImageSlot {
            use crate::preview::markdown::ImageSlot;
            let Some(font) = font else {
                return ImageSlot::Unavailable;
            };
            if let Some(p) = resolve_md_image_path(url, base_dir.as_deref()) {
                match md_image_dims(&p) {
                    Some((pw, ph)) => {
                        let (cols, rows) = md_image_cells(
                            pw,
                            ph,
                            font.width,
                            font.height,
                            max_cols.unwrap_or(avail),
                            MD_IMAGE_MAX_ROWS,
                        );
                        ImageSlot::Inline { cols, rows }
                    }
                    None => ImageSlot::Unavailable,
                }
            } else if crate::preview::markdown::is_remote_image_url(url)
                && !self.md_remote_failed.contains(url)
            {
                ImageSlot::Loading
            } else {
                ImageSlot::Unavailable
            }
        };
        // ```mermaid fence: in image mode, look up the rendered cache (content-hash key).
        // Not-yet-arrived becomes a loading line; failed/mode-off becomes the text diagram
        // (principle #3). The probe (empty string) is the representative "should this be
        // extracted?" check, so it's answered by the mode alone.
        let mermaid_on = self.mermaid_image_mode();
        let fence_target_rows = self.mermaid_fit_rows();
        let mermaid_slot = |code: &str| -> crate::preview::markdown::MermaidSlot {
            use crate::preview::markdown::MermaidSlot;
            if !mermaid_on {
                return MermaidSlot::Text;
            }
            if code.is_empty() {
                return MermaidSlot::Loading; // probe: extraction is ON
            }
            let Some(font) = font else {
                return MermaidSlot::Text;
            };
            let key = PathBuf::from(crate::preview::markdown::mermaid_fence_url(code));
            match self.md_image_cache.get(&key) {
                Some(e) if e.failed => MermaidSlot::Text,
                Some(e) => match e.decoded.as_ref() {
                    Some(img) => {
                        use image::GenericImageView;
                        // The layout is fixed at the **first** result's `layout_px` — the
                        // SVG's intrinsic size (px user units, the domain `mermaid_cells`
                        // sizes text against), not the raster's own pixel dimensions — so a
                        // sharp re-raster on zoom (higher density, same layout_px) never
                        // changes the reserved cell count. Falls back to the raster's
                        // dimensions only if intrinsic-size extraction ever failed.
                        let (pw, ph) = e.layout_px.unwrap_or_else(|| img.dimensions());
                        let (cols, rows) = mermaid_cells(
                            pw,
                            ph,
                            font.width,
                            font.height,
                            avail,
                            fence_target_rows,
                        );
                        MermaidSlot::Image { cols, rows }
                    }
                    None => MermaidSlot::Loading,
                },
                None => MermaidSlot::Loading,
            }
        };
        // Math ($…$ / $$…$$): in image mode, look up the rendered cache (a latex+display
        // hash key). Not-yet-arrived becomes loading; failed/unsupported/no font degrades
        // to raw LaTeX text (principle #3). An inline expression is lifted onto its own
        // line (a synthetic-key image, the same shape as a mermaid fence).
        let math_on = font.is_some() && self.math_image_mode();
        let math_slot = |latex: &str, display: bool| -> crate::preview::markdown::MathSlot {
            use crate::preview::markdown::MathSlot;
            let Some(font) = font else {
                return MathSlot::Raw;
            };
            let key = PathBuf::from(crate::preview::markdown::math_url(latex, display));
            match self.md_image_cache.get(&key) {
                Some(e) if e.failed => MathSlot::Raw,
                Some(e) => match (e.decoded.as_ref(), e.layout_px) {
                    // For math, layout_px holds the SVG's **intrinsic size (in em
                    // units)**, not raster px. Derive rows from the em height and columns
                    // from the intrinsic aspect ratio, for a size balanced against the text.
                    (Some(_), Some((uw, uh))) => {
                        let (cols, rows) =
                            math_cells(uw, uh, font.width, font.height, avail, display);
                        MathSlot::Image { cols, rows }
                    }
                    _ => MathSlot::Loading,
                },
                None => MathSlot::Loading,
            }
        };

        match source {
            DecoratedSource::File => self.build_decorated_file(
                path,
                width,
                code,
                &slot_of,
                mermaid_on,
                &mermaid_slot,
                math_on,
                &math_slot,
            ),
            DecoratedSource::Diff { old_pre, new_pre } => {
                // No per-ordinal `<details>` overrides make sense across two documents whose own
                // block alignment isn't known until `render_markdown_diff_aligned` runs — every
                // `<details>` block in either version simply falls back to its own `open`
                // attribute (`next_details_open`'s own "missing entries default to expanded, via
                // the attribute" contract), matching this presentation's read-only nature (there is
                // no `Space`/`Enter` toggle to persist an override for in the first place — `items`
                // is empty, so a `<details>` summary is never even focusable here).
                crate::preview::markdown::set_details_open(Vec::new());
                let (lines, images, _extras, marks) =
                    crate::preview::markdown::render_markdown_diff_aligned(
                        old_pre,
                        new_pre,
                        width,
                        code,
                        &theme.code_theme,
                        self.cfg.ui.icons,
                        &self.cfg.ui.md_task_state_chars(),
                        &slot_of,
                        &mermaid_slot,
                        tr(self.lang, crate::i18n::Msg::MermaidCaption),
                        self.cfg.ui.md_alerts,
                        &math_slot,
                        math_on,
                        self.cfg.ui.md_block_aligns(),
                    );
                // Collected from **both** versions' text — a diagram/image/equation that only
                // exists in the old (removed) or new (added) side still needs its own encode
                // request; `ensure_md_cache`'s resync loop dedups nothing, but re-requesting an
                // already-cached/in-flight key is a cheap no-op on every one of those call sites.
                let remote = if font.is_some() {
                    let mut v = crate::preview::markdown::collect_remote_image_urls(old_pre);
                    v.extend(crate::preview::markdown::collect_remote_image_urls(new_pre));
                    v
                } else {
                    Vec::new()
                };
                let fences = if mermaid_on {
                    let mut v = crate::preview::markdown::collect_mermaid_fences(old_pre);
                    v.extend(crate::preview::markdown::collect_mermaid_fences(new_pre));
                    v
                } else {
                    Vec::new()
                };
                let math = if math_on {
                    let mut v = crate::preview::markdown::collect_math_exprs(old_pre);
                    v.extend(crate::preview::markdown::collect_math_exprs(new_pre));
                    v
                } else {
                    Vec::new()
                };
                DecoratedMarkdown {
                    lines,
                    images,
                    remote_urls: remote,
                    mermaid_fences: fences,
                    math_exprs: math,
                    src_lines: new_pre.lines().count(),
                    pre_src: new_pre.to_string(),
                    pre_origin: Vec::new(),
                    extras: crate::preview::markdown::MdRenderExtras::default(),
                    diff_marks: marks,
                }
            }
        }
    }

    /// `DecoratedSource::File`'s own body — reads `path` from disk and dispatches on
    /// `self.tab.preview_kind` (Markdown/Mermaid/Code/other), exactly as `build_decorated` always
    /// did before the `Rendered` presentation started sharing this function. `code`/`slot_of`/
    /// `mermaid_on`/`mermaid_slot`/`math_on`/`math_slot` are `build_decorated`'s own shared setup,
    /// threaded straight through (never re-derived here) — see that function's own doc comment.
    #[allow(clippy::too_many_arguments)]
    fn build_decorated_file(
        &self,
        path: &Path,
        width: u16,
        code: crate::preview::markdown::CodeStyle,
        slot_of: &dyn Fn(&str, Option<u16>) -> crate::preview::markdown::ImageSlot,
        mermaid_on: bool,
        mermaid_slot: &dyn Fn(&str) -> crate::preview::markdown::MermaidSlot,
        math_on: bool,
        math_slot: &dyn Fn(&str, bool) -> crate::preview::markdown::MathSlot,
    ) -> DecoratedMarkdown {
        let src = match crate::preview::text::load(path) {
            Ok(content) => {
                let mut s = content.lines.join("\n");
                if content.truncated {
                    s.push_str("\n\n— (省略: 表示上限に達しました) —");
                }
                s
            }
            Err(e) => {
                return DecoratedMarkdown {
                    lines: vec![Line::from(format!("[can not preview: 読み込み失敗] {e}"))],
                    images: Vec::new(),
                    remote_urls: Vec::new(),
                    mermaid_fences: Vec::new(),
                    math_exprs: Vec::new(),
                    src_lines: 0,
                    pre_src: String::new(),
                    pre_origin: Vec::new(),
                    extras: crate::preview::markdown::MdRenderExtras::default(),
                    diff_marks: Vec::new(),
                }
            }
        };
        // Source line count, used to map the scroll position back to an approximate source line.
        let src_lines = src.lines().count();
        match &self.tab.preview_kind {
            Some(PreviewKind::Markdown(_)) => {
                let theme = &self.cfg.ui.theme;
                // Front matter: split the leading `---`…`---` off so the body renders normally; a dim
                // metadata block is prepended to the result below (and image line indices offset).
                let (fm_lines, src) = if self.cfg.ui.md_frontmatter {
                    match crate::preview::markdown::strip_front_matter(&src) {
                        (Some(fm), body) => (
                            crate::preview::markdown::render_front_matter(&fm, width),
                            body,
                        ),
                        (None, body) => (Vec::new(), body),
                    }
                } else {
                    (Vec::new(), src)
                };
                // Footnotes: rewrite `[^1]` refs to superscripts and pull `[^1]: …` defs into a
                // section at the end (fence-aware; no-op without definitions).
                // Each pre-pass also reports, per output line, which body line it came from, so the
                // checkbox toggle can prove the checkbox it is about to edit really is the one on
                // screen instead of trusting that the two counts tally (see `markdown::LineOrigin`).
                let origin = crate::preview::markdown::identity_origin(&src);
                let (src, origin) = if self.cfg.ui.md_footnotes {
                    crate::preview::markdown::process_footnotes_traced(&src, &origin)
                } else {
                    (src, origin)
                };
                // Inline HTML (<kbd>/<del>/<sup>/<sub>/<br>) → Markdown/Unicode konoma's own renderer draws.
                let (src, origin) = if self.cfg.ui.md_inline_html {
                    crate::preview::markdown::process_inline_html_traced(&src, &origin)
                } else {
                    (src, origin)
                };
                // Preprocessing is done: from here on `src` is exactly what the renderer parses.
                // Keep it so the source scanners (`y c`, the checkbox toggle) can read the very same
                // string instead of re-deriving this chain from the file — see `MdCache::pre_src`.
                let pre_src = src.clone();
                // Compute each `<details>` block's effective open/closed state and pass it to the
                // renderer (thread-local) + stash it so ensure_md_cache can load it onto MdCache
                // (the toggle's baseline value).
                let details_states: Vec<bool> =
                    crate::preview::markdown::collect_details_open(&src)
                        .iter()
                        .enumerate()
                        .map(|(ord, &attr)| {
                            self.details_open
                                .get(&ord)
                                .copied()
                                .unwrap_or_else(|| self.details_default_open(attr))
                        })
                        .collect();
                crate::preview::markdown::set_details_open(details_states.clone());
                let (mut lines, mut images, extras) =
                    crate::preview::markdown::render_markdown_with_images_aligned(
                        &src,
                        width,
                        code,
                        &theme.code_theme,
                        self.cfg.ui.icons,
                        &self.cfg.ui.md_task_state_chars(),
                        slot_of,
                        mermaid_slot,
                        tr(self.lang, crate::i18n::Msg::MermaidCaption),
                        self.cfg.ui.md_alerts,
                        math_slot,
                        math_on,
                        // `[ui] md_table_align` / `[ui] md_image_align`. The default pair is the
                        // historical layout (tables left, images and diagrams centered), so an
                        // unconfigured konoma draws exactly what it always did.
                        self.cfg.ui.md_block_aligns(),
                    );
                let remote = if self.picker.is_some() {
                    crate::preview::markdown::collect_remote_image_urls(&src)
                } else {
                    Vec::new()
                };
                let fences = if mermaid_on {
                    crate::preview::markdown::collect_mermaid_fences(&src)
                } else {
                    Vec::new()
                };
                let math = if math_on {
                    crate::preview::markdown::collect_math_exprs(&src)
                } else {
                    Vec::new()
                };
                // Prepend the front-matter metadata block, shifting image placements down past it.
                if !fm_lines.is_empty() {
                    for p in &mut images {
                        p.line += fm_lines.len();
                    }
                    let mut all = fm_lines;
                    all.extend(lines);
                    lines = all;
                }
                DecoratedMarkdown {
                    lines,
                    images,
                    remote_urls: remote,
                    mermaid_fences: fences,
                    math_exprs: math,
                    src_lines,
                    pre_src,
                    pre_origin: origin,
                    // `render_markdown_with_images`'s own record, straight from the same parse it
                    // rendered from — see `MdRenderExtras`'s own doc comment. `y c`/the checkbox
                    // toggle read a focused item's source straight off this list, by ordinal, with
                    // no re-scan and no count-guard reconciliation of any kind.
                    extras,
                    diff_marks: Vec::new(),
                }
            }
            Some(PreviewKind::Mermaid(_)) => DecoratedMarkdown {
                lines: crate::preview::markdown::render_mermaid_file(&src, width),
                images: Vec::new(),
                remote_urls: Vec::new(),
                mermaid_fences: Vec::new(),
                math_exprs: Vec::new(),
                src_lines,
                pre_src: String::new(),
                pre_origin: Vec::new(),
                extras: crate::preview::markdown::MdRenderExtras::default(),
                diff_marks: Vec::new(),
            },
            // A standalone code file is syntax-highlighted via syntect.
            Some(PreviewKind::Code(_)) => DecoratedMarkdown {
                lines: crate::preview::code::highlight(&src, path, &self.cfg.ui.theme.code_theme),
                images: Vec::new(),
                remote_urls: Vec::new(),
                mermaid_fences: Vec::new(),
                math_exprs: Vec::new(),
                src_lines,
                pre_src: String::new(),
                pre_origin: Vec::new(),
                extras: crate::preview::markdown::MdRenderExtras::default(),
                diff_marks: Vec::new(),
            },
            _ => DecoratedMarkdown {
                lines: Vec::new(),
                images: Vec::new(),
                remote_urls: Vec::new(),
                mermaid_fences: Vec::new(),
                math_exprs: Vec::new(),
                src_lines,
                pre_src: String::new(),
                pre_origin: Vec::new(),
                extras: crate::preview::markdown::MdRenderExtras::default(),
                diff_marks: Vec::new(),
            },
        }
    }
}

/// The default open state of a `<details>` block from `[ui] md_details` and the tag's own `open`
/// attribute: `"open"`/`"closed"` force it, `"auto"` (or anything else) honors the attribute the way
/// GitHub does.
///
/// A free function taking the config string rather than only an `&App` method, so the
/// golden-snapshot harness (`app::md_snapshot_tests::render_case_with_width`) applies **this** rule instead
/// of a hand-copied mirror of it. Until 2026-08-25 that harness computed the states straight from
/// `collect_details_open` — i.e. from the tags' own attributes, which is only what `"auto"` happens
/// to mean — so `[ui] md_details` was structurally unobservable from every golden.
pub(super) fn details_default_open(md_details: &str, open_attr: bool) -> bool {
    match md_details {
        "open" => true,
        "closed" => false,
        _ => open_attr,
    }
}
