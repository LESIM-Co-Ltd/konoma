use super::*;

impl App {
    // ---- Tabs (FR-5) ----

    /// Snapshot the current tree working state.
    pub(super) fn snapshot_tab(&self) -> PerTab {
        self.tab.clone()
    }

    /// Save the current working state to the active tab.
    pub(super) fn save_active(&mut self) {
        // This tab's media is about to be discarded by clear_image. Stash the one slot so that
        // when we return, decoding/rasterizing/launching external tools doesn't need to be redone.
        self.stash_media_cache();
        let snap = self.snapshot_tab();
        self.tabs[self.active_tab] = snap;
    }

    /// Load the active tab's snapshot into the working fields.
    /// **Also restores that tab's mode/preview** (does not drop to Tree). Images are restored by reloading.
    /// The snapshot is **moved** out of the slot (no deep clone of entries/filter_pool/git vectors):
    /// while a tab is active its slot is never read (tab_label/tab_root use the live App fields)
    /// and every switch path calls save_active before the slot is needed again.
    fn load_active(&mut self) {
        let mut t = std::mem::take(&mut self.tabs[self.active_tab]);
        // preview_path/preview_kind are read below (table-hits check, media block, setup_windowed,
        // load_table) before the bulk `self.tab = t` restore at the end of this function, so they
        // are restored early here (mirroring `self.tab.md_raw = t.md_raw;` further down).
        self.tab.preview_path = t.preview_path.clone();
        self.tab.preview_kind = t.preview_kind.clone();
        // Same reason: `kind_loads_media`/`start_media_load` below may read/overwrite it
        // (a text-mode delegated command regenerating synchronously) before `self.tab = t` restores
        // the whole bundle at the end — hoist this tab's own saved value first so that happens
        // against the *target* tab's leftover output, not whatever tab was live a moment ago.
        self.tab.command_out = t.command_out.clone();
        // root/open_dir/entries/selected/show_hidden/tree_viewport/mode/preview_scroll/
        // preview_hscroll/preview_viewport/preview_byte_top/preview_top_line/selection/visual_anchor/
        // tree_filter/filter_input/filter_pool/changed_filter/preview_search/search_input/search_idx:
        // pure per-tab copies with no read anywhere below before `self.tab = t` restores the whole
        // PerTab bundle at the end of this function — nothing in between reads them, so no individual
        // line is needed here.
        // The graph's branch-visibility picker (modal) is not carried across tabs: keep it closed
        // on restore.
        // (The git overlay itself and the graph decoration state are restored together later via
        // `self.tab = t`.)
        self.git_graph_picker = false;
        self.git_graph_picker_sel = 0;
        self.git_graph_picker_set.clear();
        self.git_graph_reordered = false;
        // The heading outline overlay is not carried across tabs either.
        self.outline_open = false;
        // Same for the table-cell full-text popup (the meaning of the shown cell changes per tab).
        self.table_cell_open = false;
        // The <details> open/closed state is also per-document = not carried across tabs.
        self.details_open.clear();
        // `table_search_hits` is a display-only set derived from `search_matches`. It isn't held in
        // PerTab (to avoid dual bookkeeping); rebuild it from the restored `search_matches` (still
        // sitting on `t`) (`self.tab` itself keeps the previous tab's values until `self.tab = t`
        // at the end, so read `t` directly here — a borrow only, `t` isn't consumed). Empty it for
        // anything but a table — otherwise another tab's matched-cell coordinates would linger and
        // the table renderer (which refers to `table_cell_is_hit` unconditionally) would highlight
        // them by mistake.
        self.table_search_hits = if matches!(
            self.tab.preview_kind,
            Some(PreviewKind::Table { .. }) | Some(PreviewKind::Archive { .. })
        ) {
            t.search_matches.iter().map(|&(_, r, c)| (r, c)).collect()
        } else {
            std::collections::HashSet::new()
        };
        // The decoration cache isn't carried over (decorated_lines regenerates it).
        self.md_cache = None;
        // A restored diff preview doesn't carry over the follow-origin mark (a session isn't a
        // cross-tab concept).
        self.diff_follow_scope = false;
        // Images don't carry over their heavy state (protocol/source image/GIF frames) — for an
        // image-type preview, restore it by reloading instead.
        // (clear_image also resets tab.image_center/tab.pdf_page/tab.pdf_pages to their defaults,
        // but `self.tab = t` at the end of this function overwrites them with the restored values,
        // so this is harmless as long as nothing in between reads them.)
        self.clear_image();
        if let (Some(kind), Some(path)) =
            (self.tab.preview_kind.clone(), self.tab.preview_path.clone())
        {
            // Mermaid (.mmd image mode) / a full-screen fence are also targets of media
            // restoration (kind_loads_media). Missing them meant nobody called start_media_load
            // after clear_image, and reload_media_if_changed also returned early on a matching
            // mtime, so returning to the tab degraded to a text diagram / a spurious error.
            if self.kind_loads_media(&kind) {
                // SVG/video thumbnail/GIF loading starts on a separate thread (set_* doesn't touch
                // zoom/center). Setting the saved values right away means the restored zoom/center
                // is kept even if the result arrives later.
                // A GIF plays from its first frame.
                // For PDF, put the saved page back before start_media_load (that generation
                // rasterizes that page).
                // Clamp `t.pdf_page` right here: use the same clamped value both as the argument
                // to restore_media_cache below and as the final value carried by `self.tab = t` at the end.
                t.pdf_page = t.pdf_page.max(1);
                // pdf_pages: a plain copy with no transformation. Not read here, so leave it to the
                // bulk restore at the end.
                // If it's the same file, same mtime, same page — e.g. returning to a tab you just
                // viewed — reuse the stashed decoded image as-is (skip re-launching external
                // tools/re-rasterizing for PDF/video).
                let reused = self.restore_media_cache(&path, t.pdf_page);
                if !reused {
                    self.start_media_load(&kind, &path);
                }
                self.tab.image_zoom = t.image_zoom;
                // image_center: a plain copy with no transformation. Not read here, so leave it to
                // the bulk restore at the end.
                self.image_crop = None;
                // Carry a synchronously-regenerated command_out (the sync-fallback path used by
                // tests without a media_tx) back into `t`, or the bulk `self.tab = t` below would
                // revert it to this tab's stale pre-restore leftover — same reasoning as the
                // `t.pdf_page` clamp above.
                t.command_out = self.tab.command_out.clone();
                if reused {
                    // Restore doesn't go through apply_payload, so the sharp reraster that would
                    // normally fire there doesn't run. This prevents an SVG/mermaid that left its
                    // tab mid-reraster from coming back still blurry (called after the zoom is
                    // restored = the needed density is judged from the restored value).
                    self.maybe_sharpen_vector();
                }
            }
        }
        // Restore the raw-source display state before re-arming windowed (order matters since raw
        // md is windowed reading). setup_windowed reads it via is_raw_source, so copy it here
        // without waiting for the bulk restore at the end
        // (it's Copy, so `t` isn't consumed, and the same value still lands on `self.tab = t` at the end).
        self.tab.md_raw = t.md_raw;
        // Tab focus / the inline diagram's zoom / full-screen-return info go to this tab's saved
        // values (never apply the previous tab's values to a different document). md_items gets
        // rebuilt by the next render's ensure_md_cache for this tab's document, and focused_item is
        // clamped to range at that point (emptied here so nothing references the old document's
        // items in the meantime).
        // (focused_item/fence_zoom/fence_center/fence_return aren't read here, so leave them to
        // `self.tab = t` at the end.)
        self.md_items.clear();
        // For a large Code/Text (+ raw Markdown/Mermaid), re-arm the windowed-reading reader
        // (byte_top is restored at the end).
        self.setup_windowed();
        // The windowed preview's 2D caret is restored by `self.tab = t` at the end (the selection
        // isn't carried over). The range is clamped on the next render/move.
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        // For a CSV/TSV table, re-parse the body and restore the saved cursor/scroll, then clamp it.
        self.load_table();
        self.tab = t;
        self.clamp_table_cursor();

        // **Reload the tab that just became active from disk.** File watching only looks at the
        // active root (rewatch), so external changes made while this tab sat in the background
        // (file create/delete/rename, git status, the change gutter/diff) are still stale in the
        // snapshot. Route it through the same refresh_fs path used for fs events, so the tree
        // rebuild, git status, diff/gutter cache, changed filter, git view, and preview reload all
        // catch up together. `false` = keep the cache for the heavy ignore set (gitignore) instead
        // of rebuilding it (if the root changes, the next render's refresh_git_if_needed handles it
        // via the per-workdir cache anyway = doesn't break the huge-repo perf work
        // [[git-watch-feedback-loop-perf]]). The preview's display position is preserved by
        // reload_preview (scroll/zoom/table cursor).
        // While this tab sat in the background it was outside fs watching, so its git status may
        // have missed an external change (for another tab on the same repo, the per-workdir cache
        // gets reused and the staleness lingers). Mark it dirty at the tab-switch checkpoint for
        // re-verification, and let the next render's refresh_git_if_needed refetch it (the same
        // shape as back_to_tree; rapid h/l presses don't mark it dirty, so that optimization is preserved).
        self.git_status_dirty = true;
        // The preview itself has already been rebuilt from disk above, so **don't reload it again**
        // here (previously refresh_fs → reload_preview repeated the same work, running the
        // full-row CSV parse twice for a CSV tab).
        let _ = self.refresh_fs_after_tab_switch();
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    /// Display name of tab `i`. While showing Tree, the **root directory name**; while showing Preview/image, etc.,
    /// the **preview target's file name**. The active tab references the latest working state (App fields),
    /// while inactive tabs reference the snapshot (`PerTab`) (since saving happens only on switch).
    pub fn tab_label(&self, i: usize) -> String {
        // Use (mode, root, preview_path) differently depending on whether this is the active tab.
        let (mode, root, preview) = if i == self.active_tab {
            (
                self.tab.mode,
                self.tab.root.as_path(),
                self.tab.preview_path.as_deref(),
            )
        } else if let Some(t) = self.tabs.get(i) {
            (t.mode, t.root.as_path(), t.preview_path.as_deref())
        } else {
            return String::new();
        };
        let path = match mode {
            Mode::Preview => preview.unwrap_or(root), // just in case: no preview falls back to root
            Mode::Tree => root,
        };
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    /// New tab. Create another tree context starting from the current root and switch to it.
    /// The new tab starts in Tree (since it has no preview target). The source tab's preview is
    /// preserved by `save_active`, so it is not lost.
    pub fn tab_new(&mut self) -> Result<()> {
        self.save_active();
        // The heading outline overlay isn't carried into the new tab (prevents an empty overlay
        // on an empty tab).
        self.outline_open = false;
        // Same for the table-cell full-text popup.
        self.table_cell_open = false;
        let root = self.tab.root.clone();
        self.tab.open_dir = root.clone();
        self.tab.root = root;
        self.tab.selected = 0;
        // A fresh tab starts from `[ui] show_hidden` again rather than silently inheriting
        // whatever the source tab had toggled `.` to — the config default is a per-tab starting
        // point, not a one-time app-startup value (matches `App::new`; session restore still wins,
        // since `apply_saved_tab` overwrites this right after `tab_new` creates the tab it restores into).
        self.tab.show_hidden = self.cfg.ui.show_hidden;
        self.tab.entries.clear();
        self.rebuild_tree()?;
        // Reset to Tree before the snapshot (order matters since the snapshot also captures
        // preview state).
        self.tab.mode = Mode::Tree;
        self.clear_image();
        self.tab.preview_path = None;
        self.tab.preview_kind = None;
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        // Just a reference reset — the file itself (if any) is still legitimately owned by the
        // *previous* tab's snapshot just taken by save_active() above, so it must not be deleted
        // here (see clear_command_out's doc comment for why this can't just call that).
        self.tab.command_out = None;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        // A new tab also starts md focus / the inline diagram zoom from scratch (part of the
        // PerTab duplication set).
        self.md_items.clear();
        self.tab.focused_item = None;
        self.tab.fence_zoom = 1.0;
        self.tab.fence_center = (0.5, 0.5);
        self.tab.fence_return = None;
        // A new tab starts as a plain Tree with no git overlay (the current tab's git state was
        // already preserved by save_active).
        self.tab.git_view = false;
        self.tab.git_view_sel = 0;
        self.tab.git_view_entries.clear();
        self.tab.came_from_git_view = false;
        self.tab.git_log = None;
        self.tab.git_log_sel = 0;
        self.tab.git_detail = None;
        self.tab.git_detail_meta = None;
        self.tab.git_detail_title = None;
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_viewport = 0;
        self.tab.git_detail_total = 0;
        self.tab.git_branches = None;
        self.tab.git_branch_sel = 0;
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_filtering = false;
        self.tab.git_worktrees = None;
        self.tab.git_worktree_sel = 0;
        self.tab.git_worktree_filter.clear();
        self.tab.git_worktree_filtering = false;
        self.tab.git_graph = None;
        self.tab.git_graph_sel = 0;
        // A new tab also starts the graph's decoration state (base/legend/visible branches/order)
        // and picker from scratch (the current tab's were already preserved in PerTab by the
        // preceding save_active). They get re-derived the next time it's opened.
        self.tab.git_graph_base = None;
        self.tab.git_graph_base_label = None;
        self.tab.git_graph_visible.clear();
        self.tab.git_graph_legend.clear();
        self.tab.git_graph_hidden = 0;
        self.tab.git_graph_order.clear();
        self.git_graph_picker = false;
        self.git_graph_picker_sel = 0;
        self.git_graph_picker_set.clear();
        self.git_graph_reordered = false;
        // A new tab starts selection / filter / preview search empty
        // (the current tab's were already preserved in PerTab by the preceding save_active).
        self.tab.selection.clear();
        self.tab.visual_anchor = None;
        self.clear_filter_state();
        self.search_clear();
        self.tabs.push(self.snapshot_tab());
        self.active_tab = self.tabs.len() - 1;
        // Save the session at the checkpoint where the tab set changes (the most recent tab
        // layout survives even an abnormal exit).
        self.save_session();
        Ok(())
    }

    /// `Ctrl-t` in the tree: open the entry under the cursor in a new (foreground) tab, leaving the
    /// current tab untouched. A file opens as a preview; a directory becomes the new tab's root
    /// (mirroring `l`). No-op when the tree is empty.
    pub fn tab_new_from_selection(&mut self) -> Result<()> {
        let Some(entry) = self.tab.entries.get(self.tab.selected).cloned() else {
            return Ok(());
        };
        self.tab_new()?; // fresh Tree tab at the current root, now active
        if entry.is_dir {
            // Descend into the folder in the new tab (same as `l`); root changes, so drop stale state.
            self.clear_for_root_change();
            self.tab.root = entry.path;
            self.tab.entries.clear();
            self.tab.selected = 0;
            self.rebuild_tree()?;
        } else {
            // Reveal the file (so returning with `q` lands on it), then preview it.
            let _ = self.reveal_path_deep(&entry.path);
            self.enter_preview(&entry.path);
        }
        self.save_session();
        Ok(())
    }

    /// Close the active tab. The last one is not closed.
    pub fn tab_close(&mut self) {
        if self.tabs.len() <= 1 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CantCloseLastTab).into());
            return;
        }
        // Nobody will come back to the tab that's closing, so its media is done for. If the
        // one-slot cache kept holding it, an image unrelated to any tab would stay resident.
        self.drop_media_cache_for_active();
        // Same for a live delegated-command temp output — this tab's own slot in `self.tabs` is
        // about to be removed below without ever being saved again, so nothing else will clean it up.
        self.clear_command_out();
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.load_active();
        self.save_session();
    }

    /// Move tabs relatively (dir: +1=next / -1=previous). Wraps at the ends.
    pub fn tab_cycle(&mut self, dir: i32) {
        self.tab_list = false; // switching via [/] from the list also means the list is done
        if self.tabs.len() <= 1 {
            return;
        }
        self.save_active();
        let n = self.tabs.len() as i32;
        self.active_tab = (self.active_tab as i32 + dir).rem_euclid(n) as usize;
        self.load_active();
        self.save_session();
    }

    /// Select a tab by number (0-based). Out-of-range or the same tab is ignored.
    pub fn tab_goto(&mut self, i: usize) {
        self.tab_list = false; // switching from the list (1-9/Enter) closes the list
        if i >= self.tabs.len() || i == self.active_tab {
            return;
        }
        self.save_active();
        self.active_tab = i;
        self.load_active();
        self.save_session();
    }

    // --- Tab-list overlay (`T`) ----------------------------------------
    pub fn is_tab_list(&self) -> bool {
        self.tab_list
    }
    /// `T`: open/close the tab list. Opening puts the selection on the active tab.
    pub fn toggle_tab_list(&mut self) {
        self.tab_list = !self.tab_list;
        if self.tab_list {
            self.tab_list_sel = self.active_tab;
        }
    }
    pub fn tab_list_sel(&self) -> usize {
        self.tab_list_sel
    }
    pub fn tab_list_move(&mut self, delta: i32) {
        let n = self.tabs.len();
        if n == 0 {
            return;
        }
        self.tab_list_sel = (self.tab_list_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }
    /// Enter in the list: switch to the selected tab (closing the list; same tab = just close).
    pub fn tab_list_activate(&mut self) {
        let i = self.tab_list_sel;
        self.tab_list = false;
        self.tab_goto(i);
    }
    /// `w` in the list: close the **selected** tab (the list stays open, like the bookmark list).
    /// Closing the active tab switches to a neighbor; the last tab refuses with a flash.
    pub fn tab_list_close_selected(&mut self) {
        if self.tabs.len() <= 1 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CantCloseLastTab).into());
            return;
        }
        let i = self.tab_list_sel;
        if i == self.active_tab {
            self.tab_close();
        } else {
            // If the cache is holding the media of the (inactive) tab being closed, release it.
            if let Some(t) = self.tabs.get(i) {
                let closing = t.preview_path.clone();
                if matches!((&self.media_cache, &closing), (Some(c), Some(p)) if &c.path == p) {
                    self.media_cache = None;
                }
                // Likewise, delete that (inactive) tab's own delegated-command temp output — its
                // slot in `self.tabs` is removed below without ever becoming active again, so
                // `clear_command_out` (which only ever acts on the *live* `self.tab`) can't reach it.
                if let Some(p) = &t.command_out {
                    let _ = std::fs::remove_file(p);
                }
            }
            self.tabs.remove(i);
            if self.active_tab > i {
                self.active_tab -= 1;
            }
        }
        self.tab_list_sel = self.tab_list_sel.min(self.tabs.len() - 1);
        self.save_session();
    }
    /// Root directory of tab `i` (for the tab list's path column).
    pub fn tab_root(&self, i: usize) -> std::path::PathBuf {
        if i == self.active_tab {
            self.tab.root.clone()
        } else {
            self.tabs
                .get(i)
                .map(|t| t.root.clone())
                .unwrap_or_else(|| self.tab.root.clone())
        }
    }
}
