use super::*;

impl App {
    /// Build (or reuse) the decorated cache for a Markdown/Mermaid/code preview at display width
    /// `width`. On a miss (file/width change) this renders, collapses links, collects the Tab items,
    /// and precomputes the wrap layout (row prefix sums via ratatui's own per-line reflow) — all the
    /// per-document work, done once. Frames then only slice the visible range (`md_slice`).
    pub(super) fn ensure_md_cache(&mut self, width: u16) {
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        // If the fence diagram's target row count (fit-to-view) changed, only rebuild documents
        // that contain a diagram (a viewport height change does not rebuild every md every time).
        let fence_rows = self.mermaid_fit_rows();
        if matches!(&self.md_cache, Some(c) if c.path == path && c.width == width
            && (c.fence_rows == fence_rows
                || !c.images.iter().any(|p| crate::preview::markdown::is_mermaid_fence_url(&p.url))))
        {
            return;
        }
        let decorated = self.build_decorated(&path, width);
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
        // line re-lays out into the real inline diagram — the remote-image pattern exactly.
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
        let decorated = if resync {
            self.build_decorated(&path, width)
        } else {
            decorated
        };
        let (lines, targets) = self.postprocess_md(decorated.lines);
        // Pass the drawn mermaid placements' source ordinal (document order) for sentinel matching.
        let fence_ords: Vec<usize> = decorated
            .images
            .iter()
            .filter_map(|p| p.fence_ord)
            .collect();
        let items = build_md_items(
            &lines,
            &targets,
            &fence_ords,
            &decorated.code_blocks,
            &decorated.tasks,
        );
        let anchors = compute_md_anchors(&lines);
        let max_line_cols = lines.iter().map(|l| l.width()).max().unwrap_or(0);
        let row_prefix = if self.cfg.ui.wrap && width > 0 {
            use ratatui::text::Text;
            use ratatui::widgets::{Paragraph, Wrap};
            let mut pre = Vec::with_capacity(lines.len() + 1);
            let mut acc = 0usize;
            pre.push(0);
            for line in &lines {
                acc += Paragraph::new(Text::from(vec![line.clone()]))
                    .wrap(Wrap { trim: false })
                    .line_count(width)
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
        });
    }

    /// The default open state of a `<details>` block from `ui.md_details` and its `open` attribute:
    /// `"open"`/`"closed"` force it; `"auto"` (or anything else) honors the attribute like GitHub.
    fn details_default_open(&self, open_attr: bool) -> bool {
        match self.cfg.ui.md_details.as_str() {
            "open" => true,
            "closed" => false,
            _ => open_attr,
        }
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
    fn build_decorated(&self, path: &Path, width: u16) -> DecoratedMarkdown {
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
                    code_blocks: Vec::new(),
                    tasks: Vec::new(),
                }
            }
        };
        // Source line count, used to map the scroll position back to an approximate source line.
        let src_lines = src.lines().count();
        match &self.tab.preview_kind {
            Some(PreviewKind::Markdown(_)) => {
                let theme = &self.cfg.ui.theme;
                let code = crate::preview::markdown::CodeStyle {
                    bg: theme.code_bg(),
                    label_bg: theme.code_label_bg(),
                    label_right: theme.code_label_right(),
                    tab_width: self.cfg.ui.tab_width,
                    wrap: self.cfg.ui.wrap,
                };
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
                // Inline HTML (<kbd>/<del>/<sup>/<sub>/<br>) → Markdown/Unicode tui-markdown renders.
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
                // Decide how to render each image URL. A local file or a cached remote fetch resolves to
                // a path → Inline (with its display size in cells). An uncached remote URL is Loading
                // (a fetch is kicked off separately, in `decorated_lines`) unless it has already failed.
                // Anything else (no backend / missing file / data: URL) degrades to text (principle #3).
                let font = self.picker.as_ref().map(|p| p.font_size());
                let base_dir = path.parent().map(|p| p.to_path_buf());
                let avail = width.saturating_sub(2);
                let slot_of = |url: &str| -> crate::preview::markdown::ImageSlot {
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
                                    avail,
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
                                // The layout is fixed at **the first raster's dimensions** (layout_px):
                                // even when a sharp re-raster raises the density on zoom, the
                                // reserved cell count = display size stays unchanged.
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
                let math_slot =
                    |latex: &str, display: bool| -> crate::preview::markdown::MathSlot {
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
                let (mut lines, mut images, extras) =
                    crate::preview::markdown::render_markdown_with_images(
                        &src,
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
                    );
                // `render_markdown_with_images`'s own record, straight from the same parse it
                // rendered from — see `MdRenderExtras`'s own doc comment. `y c`/the checkbox toggle
                // read a focused item's source straight off this list, by ordinal, with no re-scan
                // and no count-guard reconciliation of any kind.
                let (code_blocks, tasks_at) = (extras.code_blocks, extras.tasks);
                let remote = if font.is_some() {
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
                    code_blocks,
                    tasks: tasks_at,
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
                code_blocks: Vec::new(),
                tasks: Vec::new(),
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
                code_blocks: Vec::new(),
                tasks: Vec::new(),
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
                code_blocks: Vec::new(),
                tasks: Vec::new(),
            },
        }
    }
}
