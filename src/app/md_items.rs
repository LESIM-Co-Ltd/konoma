use super::*;

impl App {
    /// Arrange the links in decorated Markdown lines to "show label only", build `md_items` (links +
    /// task checkboxes), and return the line list with the focused item rendered inverted (called just
    /// before rendering). Since tui-markdown outputs the "label (URL)" form, `collapse_links` folds the
    /// accompanying URL (the URL is the hidden destination) and styles the label as a link plus (when
    /// configured) an icon. Checkbox markers are recognized by their dedicated style (`is_task_span`).
    /// Test-only equivalent of the production path (`ensure_md_cache` + `md_slice`) over caller-
    /// supplied lines: collapse links, rebuild `md_items`, and invert the focused item. Shares
    /// `build_md_items` / `invert_focused_line` with the cache path so the two cannot drift.
    /// Post-process decorated Markdown lines just before caching/rendering: collapse the `label
    /// (URL)` links tui-markdown emits, then (when `ui.md_autolink`) auto-link bare URLs/emails, then
    /// (when `ui.md_emoji`) convert `:shortcode:` emoji. Shared by the cache build (`ensure_md_cache`)
    /// and the test-only decorate path so link/emoji collection cannot drift between them. Emoji runs
    /// last so width-affecting substitutions are reflected in the caller's reflow/`row_prefix`.
    pub(super) fn postprocess_md(
        &self,
        lines: Vec<Line<'static>>,
    ) -> (Vec<Line<'static>>, Vec<String>) {
        let (lines, targets) = collapse_links(lines, self.cfg.ui.icons);
        let (lines, targets) = if self.cfg.ui.md_autolink {
            autolink_bare_urls(lines, targets)
        } else {
            (lines, targets)
        };
        let lines = if self.cfg.ui.md_emoji {
            substitute_emoji(lines)
        } else {
            lines
        };
        (lines, targets)
    }

    #[cfg(test)]
    pub fn decorate_md_items(&mut self, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        let (lines, targets) = self.postprocess_md(lines);
        let items = build_md_items(&lines, &targets, &[]);
        // Clamp the focus index into range.
        match self.tab.focused_item {
            Some(_) if items.is_empty() => self.tab.focused_item = None,
            Some(f) if f >= items.len() => self.tab.focused_item = Some(items.len() - 1),
            _ => {}
        }
        self.md_items = items;

        let Some(target_idx) = self.tab.focused_item else {
            return lines;
        };
        let (target_line, whole_line) = {
            let it = &self.md_items[target_idx];
            (it.line, matches!(it.kind, MdItemKind::CodeBlock))
        };
        let first_on_line = self.md_items.partition_point(|x| x.line < target_line);
        let ordinal = target_idx - first_on_line;
        lines
            .into_iter()
            .enumerate()
            .map(|(li, line)| {
                if li == target_line {
                    invert_focused_line(&line, ordinal, whole_line)
                } else {
                    line
                }
            })
            .collect()
    }

    /// Move the item focus of the Markdown preview (dir: +1=next / -1=previous, cyclic over links and
    /// checkboxes in document order). If the focused line is off-screen, scroll until it is visible.
    pub fn md_focus_move(&mut self, dir: i32) {
        if self.md_items.is_empty() {
            return;
        }
        let n = self.md_items.len() as i32;
        let next = match self.tab.focused_item {
            Some(f) => (f as i32 + dir).rem_euclid(n),
            None if dir >= 0 => 0,
            None => n - 1,
        } as usize;
        if self.tab.focused_item != Some(next) {
            // When focus moves, reset the inline diagram's zoom/pan (don't carry over the previous
            // diagram's state).
            self.tab.fence_zoom = 1.0;
            self.tab.fence_center = (0.5, 0.5);
        }
        self.tab.focused_item = Some(next);
        // Bring the focused line into the visible range. `preview_scroll` is in the **visual line
        // (post-wrap)** coordinate system (the render side clamps it with `para.line_count`), so
        // the item's logical line also has to be converted to a visual line before comparing —
        // comparing in logical-line terms drifts under wrapping and fails to follow an off-screen
        // focus (2026-07-08 user report).
        let line = self.md_items[next].line;
        let (top, mut height) = self.md_visual_span(line);
        // Fenced diagram: bring not just the caption but the **whole diagram block** (caption +
        // reserved rows + bottom margin) into view (prevents "I focused it but can't see it" when
        // the diagram is cut off at the bottom — per user request).
        let block_end = if let MdItemKind::MermaidFence { ordinal } = self.md_items[next].kind {
            // The diagram has reserved rows and isn't laid out as body lines, so derive the end
            // from the placement's row count.
            self.mermaid_placement(ordinal)
                .map(|(pl, pr)| pl + pr as usize) // bottom-margin row (placement.line + rows)
                .unwrap_or(line)
        } else {
            // Code blocks / open <details> span multiple lines. Bringing only the header line into
            // view leaves the block pinned to the bottom edge with its contents hidden, so bring
            // the whole block into view instead (per user request).
            self.md_item_block_end(next)
        };
        if block_end > line {
            let (bt, bh) = self.md_visual_span(block_end);
            height = (bt + bh).saturating_sub(top).max(height);
        }
        let vh = self.tab.preview_viewport.max(1) as usize;
        let scroll = self.tab.preview_scroll as usize;
        if height >= vh || top < scroll {
            // The block is bigger than the screen (show the top), or it is cut off above.
            self.tab.preview_scroll = top as u16;
        } else if top + height > scroll + vh {
            self.tab.preview_scroll = (top + height).saturating_sub(vh) as u16;
        }
    }

    /// Last decorated line belonging to the focused item's **block**.
    ///
    /// Tab focus lands on a single line — a code block's header, a `<details>` summary — but the thing
    /// the user wants to look at continues below it. Ensuring only the focused line is on screen parks
    /// a block at the bottom edge with its contents cut off. Everything that spans more than one line
    /// reports its real extent here so `md_focus_move` can bring the whole block into view.
    fn md_item_block_end(&self, idx: usize) -> usize {
        let Some(item) = self.md_items.get(idx) else {
            return 0;
        };
        let start = item.line;
        let Some(cache) = self.md_cache.as_ref() else {
            return start;
        };
        // Whether a line is a continuation is judged by the "left band" the renderer draws (code is
        // `▎`, details body is `▏`). Since it looks at the rendered line itself, there's no need to
        // re-interpret the source.
        let belongs: fn(&ratatui::text::Line<'_>) -> bool = match item.kind {
            MdItemKind::CodeBlock => crate::preview::markdown::is_code_line,
            MdItemKind::Details { .. } => crate::preview::markdown::is_details_body_line,
            // Links/checkboxes are one line. A fenced diagram has reserved rows, so its end is
            // derived from the placement below instead.
            _ => return start,
        };
        let mut end = start;
        for (i, line) in cache.lines.iter().enumerate().skip(start + 1) {
            if !belongs(line) {
                break;
            }
            end = i;
        }
        end
    }

    #[cfg(test)]
    pub fn md_item_block_end_for_test(&self, idx: usize) -> usize {
        self.md_item_block_end(idx)
    }
    #[cfg(test)]
    pub fn md_item_line_for_test(&self, idx: usize) -> usize {
        self.md_items[idx].line
    }
    #[cfg(test)]
    pub fn md_visual_span_for_test(&self, line: usize) -> (usize, usize) {
        self.md_visual_span(line)
    }
    #[cfg(test)]
    pub fn preview_viewport_for_test(&self) -> u16 {
        self.tab.preview_viewport
    }

    /// (line, rows) of the inline mermaid placement whose **source ordinal** is `ordinal`
    /// (placements carry it in `fence_ord`; counting rendered placements would drift as soon as
    /// an earlier fence has no placement — loading or degraded to text).
    pub(super) fn mermaid_placement(&self, ordinal: usize) -> Option<(usize, u16)> {
        let cache = self.md_cache.as_ref()?;
        cache
            .images
            .iter()
            .find(|p| p.fence_ord == Some(ordinal))
            .map(|p| (p.line, p.rows))
    }

    /// Ordinal of the focused inline mermaid diagram, if the focus is on one (border cue in the renderer).
    pub fn focused_mermaid_ordinal(&self) -> Option<usize> {
        let f = self.tab.focused_item?;
        match self.md_items.get(f)?.kind {
            MdItemKind::MermaidFence { ordinal } => Some(ordinal),
            _ => None,
        }
    }

    /// Visual (post-wrap) row offset of decorated line `line` plus its own wrapped height.
    /// With wrap off this is just (line, 1). Reads the cached per-line reflow prefix sums
    /// (ratatui's own `line_count`, computed once at cache build), so the numbers match what
    /// the renderer draws exactly — and the lookup is O(1) instead of re-flowing the document.
    pub(crate) fn md_visual_span(&self, line: usize) -> (usize, usize) {
        if !self.cfg.ui.wrap {
            return (line, 1);
        }
        let Some(cache) = &self.md_cache else {
            return (line, 1);
        };
        if cache.width == 0
            || line >= cache.lines.len()
            || cache.row_prefix.len() != cache.lines.len() + 1
        {
            return (line, 1);
        }
        let top = cache.row_prefix[line];
        let height = (cache.row_prefix[line + 1] - top).max(1);
        (top, height)
    }

    /// Activate the focused item: a link opens (URLs externally, local paths within konoma),
    /// a task checkbox toggles.
    pub fn md_activate_focused(&mut self) -> Result<()> {
        let Some(f) = self.tab.focused_item else {
            return Ok(());
        };
        match self.md_items.get(f) {
            Some(MdItem {
                kind: MdItemKind::Link { target },
                ..
            }) => {
                let target = target.clone();
                self.open_link_target(&target)
            }
            Some(MdItem {
                kind: MdItemKind::Task { .. },
                ..
            }) => {
                self.md_toggle_focused_task();
                Ok(())
            }
            // A code block has no "open/toggle" target, so Enter does nothing.
            // Copying it aligns with the other copy operations and goes through `y` (the copy
            // leader, pre-empted in main.rs).
            Some(MdItem {
                kind: MdItemKind::CodeBlock,
                ..
            }) => Ok(()),
            // Inline mermaid diagram: open it full screen (with zoom/pan).
            Some(MdItem {
                kind: MdItemKind::MermaidFence { ordinal },
                ..
            }) => {
                let ord = *ordinal;
                self.open_mermaid_fence(ord);
                Ok(())
            }
            // A <details> summary: toggle the collapse.
            Some(MdItem {
                kind: MdItemKind::Details { ordinal },
                ..
            }) => {
                let ord = *ordinal;
                self.toggle_details(ord);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Open the Nth ```mermaid fence of the current Markdown preview full screen (Enter on a
    /// Tab-focused inline diagram). The Markdown view's scroll/focus are stashed so `q` returns
    /// exactly where the reader was. Count-guarded re-extraction: if the file changed and the
    /// fence is gone, flash instead of rendering a stale diagram (principle #3).
    fn open_mermaid_fence(&mut self, ordinal: usize) {
        let Some(md) = self.tab.preview_path.clone() else {
            return;
        };
        if self.mermaid_fence_code(&md, ordinal).is_none() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::DiagramOpenFailed).into());
            return;
        }
        self.tab.fence_return = Some((self.tab.preview_scroll, self.tab.focused_item));
        self.clear_image();
        let kind = PreviewKind::MermaidFence(ordinal);
        self.start_media_load(&kind, &md);
        self.tab.preview_kind = Some(kind);
    }

    /// Raw source of the focused code block, matched by ordinal against the on-screen headers.
    /// `None` when the focused item is not a code block, there is no render cache, or the counts
    /// disagree (pathological doc) — the caller then flashes instead of copying garbage (safe
    /// fallback, #3). Also used by tests to assert the copied value without a clipboard round-trip.
    ///
    /// Scans `md_cache.pre_src` — **the exact text the renderer parsed** — not a fresh read of the
    /// file. Root cause this fixed (2026-08, reported by a user on the released build): re-reading
    /// the file here meant scanning text the pre-passes had not been applied to, so any document
    /// whose structure those passes change (a multi-line footnote definition, a `<br>`) made the
    /// scanner and the screen disagree about how many code blocks exist, and the count guard refused
    /// `y c` for the *whole* document. Reading the renderer's own string removes the disagreement by
    /// construction rather than by keeping a second derivation in sync.
    ///
    /// Copying the preprocessed text is safe because the pre-passes leave code verbatim (they all
    /// gate on `literal_code_mask`): measured over 2,182 real `.md` files, the pre-passes changed
    /// 2,105 of them, and the extracted block contents differed in only the four that this very bug
    /// broke. There is no write-back here — only a copy — so, unlike `md_tasks.rs`, no byte offsets
    /// into the on-disk file are needed.
    fn focused_code_source(&self) -> Option<String> {
        let f = self.tab.focused_item?;
        if !matches!(
            self.md_items.get(f),
            Some(MdItem {
                kind: MdItemKind::CodeBlock,
                ..
            })
        ) {
            return None;
        }
        // Which code block this is within the document (an ordinal excluding links/tasks).
        let ordinal = self.md_items[..=f]
            .iter()
            .filter(|it| matches!(it.kind, MdItemKind::CodeBlock))
            .count()
            .saturating_sub(1);
        let total = self
            .md_items
            .iter()
            .filter(|it| matches!(it.kind, MdItemKind::CodeBlock))
            .count();
        // The renderer's own text (already capped by `preview::text::load` and front-matter-stripped
        // upstream), plus the `<details>` open/closed states of that same render — without those,
        // fences inside a collapsed block would be counted and the count guard would trip.
        let cache = self.md_cache.as_ref()?;
        let blocks =
            crate::preview::markdown::code_block_source_locs(&cache.pre_src, &cache.details_states);
        // Only trust and copy it when the on-screen header count matches the source's block count.
        (blocks.len() == total)
            .then(|| blocks.get(ordinal).cloned())
            .flatten()
    }

    /// Copy the focused code block's raw source to the clipboard, or flash a safe notice when it
    /// cannot be resolved (`focused_code_source` = None). Triggered by the copy leader (`y`) in a
    /// Markdown preview when a code block is focused (pre-empted in `handle_key`).
    pub fn md_copy_focused_code(&mut self) {
        let Some(text) = self.focused_code_source() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CodeBlockCopyUnavailable).into());
            return;
        };
        match set_clipboard(&text) {
            Ok(()) => self.flash = Some(tr(self.lang, crate::i18n::Msg::CopiedCodeBlock).into()),
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::CopyFailed)
                ))
            }
        }
    }

    /// Test-only: the exact text `Enter` would copy for the focused code block (no clipboard).
    #[cfg(test)]
    pub fn focused_code_text(&self) -> Option<String> {
        self.focused_code_source()
    }

    /// Whether the currently focused Markdown item is a code block (drives the footer hint:
    /// `Enter` copies the block).
    pub fn md_focused_code(&self) -> bool {
        !self.is_raw_source()
            && self
                .tab
                .focused_item
                .and_then(|f| self.md_items.get(f))
                .is_some_and(|it| matches!(it.kind, MdItemKind::CodeBlock))
    }

    /// Whether the currently focused Markdown item is a task checkbox (drives the Space fixed key:
    /// only then does Space toggle instead of falling through to the keymap).
    pub fn md_focused_task(&self) -> bool {
        !self.is_raw_source()
            && self
                .tab
                .focused_item
                .and_then(|f| self.md_items.get(f))
                .is_some_and(|it| matches!(it.kind, MdItemKind::Task { .. }))
    }

    /// The ordinal of the focused `<details>` summary, if one is focused (drives the Space fixed key
    /// and the footer hint). None when not focused on a details summary.
    pub fn md_focused_details(&self) -> Option<usize> {
        if self.is_raw_source() {
            return None;
        }
        match self.tab.focused_item.and_then(|f| self.md_items.get(f)) {
            Some(MdItem {
                kind: MdItemKind::Details { ordinal },
                ..
            }) => Some(*ordinal),
            _ => None,
        }
    }

    /// Whether the rendered Markdown preview has any task checkboxes (drives the footer hint).
    pub fn md_has_tasks(&self) -> bool {
        self.md_items
            .iter()
            .any(|it| matches!(it.kind, MdItemKind::Task { .. }))
    }

    /// Open a link target. `scheme://`/`mailto:`, etc. are delegated externally; local paths open in konoma
    /// (file=preview / directory=make it the new root and Tree).
    pub(super) fn open_link_target(&mut self, target: &str) -> Result<()> {
        let t = target.trim();
        if t.contains("://") || t.starts_with("mailto:") || t.starts_with("tel:") {
            return self.open_external(t);
        }
        if let Some(anchor) = t.strip_prefix('#') {
            if !self.md_scroll_to_anchor(anchor) {
                self.flash = Some(format!(
                    "{}{}",
                    tr(self.lang, crate::i18n::Msg::AnchorNotFound),
                    t
                ));
            }
            return Ok(());
        }
        let path_part = t.split('#').next().unwrap_or(t);
        let resolved = self.resolve_link_local(t);
        if resolved.is_dir() {
            self.back_to_tree();
            // Since the root is changing, discard the old root's selection/visual/filter/search
            // state (don't carry it over).
            self.clear_for_root_change();
            self.tab.root = resolved;
            self.tab.open_dir = self.tab.root.clone();
            self.tab.entries.clear();
            self.tab.selected = 0;
            self.rebuild_tree()?;
        } else if resolved.is_file() {
            self.enter_preview(&resolved);
        } else {
            self.flash = Some(format!(
                "{}{}",
                tr(self.lang, crate::i18n::Msg::NotFound),
                path_part
            ));
        }
        Ok(())
    }

    /// Resolve a local (non-URL, non-anchor) Markdown link target to an absolute path (existence not
    /// guaranteed), relative to the current preview file's directory. A `#anchor` suffix is dropped.
    fn resolve_link_local(&self, target: &str) -> PathBuf {
        let t = target.trim();
        let path_part = t.split('#').next().unwrap_or(t);
        let base = self
            .tab
            .preview_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.tab.root.clone());
        let p = Path::new(path_part);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        };
        std::fs::canonicalize(&resolved).unwrap_or(resolved)
    }

    /// `Ctrl-t`: open the focused Markdown link in a **new tab** (the current doc's tab stays intact).
    /// URLs still go to the browser (a tab makes no sense for them); a local file/dir opens in a fresh
    /// foreground tab via the shared paste-jump navigation (reveal + root-switch-if-outside + preview).
    /// No-op when the focused item is not a link (task/code) or nothing is focused.
    pub fn md_open_focused_link_new_tab(&mut self) -> Result<()> {
        let Some(f) = self.tab.focused_item else {
            return Ok(());
        };
        let target = match self.md_items.get(f) {
            Some(MdItem {
                kind: MdItemKind::Link { target },
                ..
            }) => target.clone(),
            _ => return Ok(()),
        };
        let t = target.trim();
        // URL/mailto/tel can't become a tab, so open it externally (browser, etc.) just like Enter.
        if t.contains("://") || t.starts_with("mailto:") || t.starts_with("tel:") {
            return self.open_external(t);
        }
        if t.starts_with('#') {
            // A same-document anchor makes no sense in a new tab — scroll in place instead.
            return self.open_link_target(t);
        }
        // Resolve the local path relative to the current md file's location before creating the tab.
        let resolved = self.resolve_link_local(t);
        if !resolved.exists() {
            self.flash = Some(format!(
                "{}{}",
                tr(self.lang, crate::i18n::Msg::NotFound),
                t.split('#').next().unwrap_or(t)
            ));
            return Ok(());
        }
        // Create a new (foreground) tab and jump inside it. The original document's tab is already
        // preserved via save_active.
        self.tab_new()?;
        self.paste_jump_to(&resolved, None);
        Ok(())
    }

    /// Open a URL/file with the OS handler (`open` on macOS, `xdg-open` elsewhere). The result is
    /// reported via flash. No-op (flash) when `[external] open_links = false`.
    fn open_external(&mut self, url: &str) -> Result<()> {
        if !self.cfg.external.open_links {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::ExternalOpenLinksDisabled).into());
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(not(target_os = "macos"))]
        let opener = "xdg-open";
        match std::process::Command::new(opener).arg(url).spawn() {
            Ok(_) => self.flash = Some(format!("{}{url}", tr(self.lang, crate::i18n::Msg::Opened))),
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::OpenFailed)
                ))
            }
        }
        Ok(())
    }
}
