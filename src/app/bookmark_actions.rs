//! Bookmarks: set/jump leader state and the bookmark-list overlay — methods on `App`.

use super::*;

impl App {
    // --- Bookmarks (M7 auxiliary) ----------------------------------------------
    /// Whether we are waiting for an alphabetic key after `m` (while true, main intercepts keys).
    pub fn is_marking(&self) -> bool {
        self.mark_set_pending
    }
    /// `m`=enter set mode (the next letter registers; scope = letter case).
    pub fn start_mark_set(&mut self) {
        self.mark_set_pending = true;
    }
    /// Cancel the mark wait (Esc or a non-applicable key).
    pub fn cancel_mark(&mut self) {
        self.mark_set_pending = false;
    }
    /// The one key after `m`: register the cursor item (or the current root) under that letter.
    /// If `ui.confirm_bookmark_overwrite` is on and the letter already points to a **different** path,
    /// open a yes/no confirmation dialog instead of overwriting silently.
    pub fn mark_input(&mut self, c: char) {
        if !std::mem::take(&mut self.mark_set_pending) {
            return;
        }
        if !c.is_ascii_alphabetic() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::InvalidMarkKey).into());
            return;
        }
        let target = self.bookmark_target();
        // Overwrite confirmation: if that key is already assigned to a **different** path and
        // confirmation is enabled, show a confirmation dialog instead of overwriting directly
        // (re-registering the same path, or an unused key, registers unconditionally = no needless confirmation).
        if self.cfg.ui.confirm_bookmark_overwrite {
            if let Some(existing) = self.bookmarks.get(c) {
                if existing != target {
                    let msg = format!(
                        "{} {c}\n{}\n  → {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkOverwriteConfirm),
                        self.bookmark_display_path(c.is_ascii_lowercase(), &existing),
                        self.bookmark_display_path(c.is_ascii_lowercase(), &target),
                    );
                    self.dialog = Some(Dialog {
                        op: PendingOp::BookmarkOverwrite { key: c, target },
                        kind: DialogKind::Confirm {
                            message: msg,
                            allow_permanent: false,
                        },
                    });
                    return;
                }
            }
        }
        self.perform_mark_set(c, target);
    }

    /// Actually write the bookmark under `c` and flash the result. Used by `mark_input` directly (no
    /// overwrite / confirm off) and by the overwrite confirmation (`dialog_confirm`).
    pub(super) fn perform_mark_set(&mut self, c: char, target: std::path::PathBuf) {
        match self.bookmarks.set(c, target.clone()) {
            Ok(_) => {
                let scope = if c.is_ascii_uppercase() {
                    crate::i18n::tr(self.lang, crate::i18n::Msg::GlobalApp)
                } else {
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Local)
                };
                self.flash = Some(format!(
                    "{} {c} = {}  [{scope}]",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Bookmarked),
                    self.bookmark_display_path(c.is_ascii_lowercase(), &target),
                ));
            }
            // Save failed: notify that it's registered in memory but will vanish on restart (don't swallow the error).
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
            }
        }
    }
    /// What `m` registers: the previewed file in Preview mode, otherwise the tree cursor entry
    /// (fallback: the current root). Preview mode's tree cursor can lag behind the shown file
    /// (bookmark jumps / follow mode open previews without moving it), so the shown file wins.
    fn bookmark_target(&self) -> std::path::PathBuf {
        if matches!(self.tab.mode, Mode::Preview) {
            if let Some(p) = &self.tab.preview_path {
                return p.clone();
            }
        }
        self.tab
            .entries
            .get(self.tab.selected)
            .map(|e| e.path.clone())
            .unwrap_or_else(|| self.tab.root.clone())
    }

    /// Display form for a bookmark target. Local marks show the contextual (relative-style) path;
    /// global marks always show their absolute location (`~`-shortened) — a global target usually
    /// lives outside the current tree, where a relative `../../...` rendering is unreadable.
    pub fn bookmark_display_path(&self, is_local: bool, path: &std::path::Path) -> String {
        if is_local {
            self.format_path(path)
        } else {
            home_relative(path)
        }
    }

    /// A plain letter inside the bookmark list (`'` opens it): jump straight to that bookmark
    /// (a-z local / A-Z global; dir=new root / file=preview). Unknown letters just flash and the
    /// list stays open. Keys claimed by the list/global keymap (j/k/q, tab keys …) never reach here.
    pub fn bookmark_jump_letter(&mut self, c: char) {
        match self.bookmarks.get(c) {
            Some(p) if p.is_dir() => {
                self.close_bookmark_list();
                self.jump_to_dir(p);
            }
            Some(p) if p.is_file() => {
                self.close_bookmark_list();
                self.enter_preview(&p);
            }
            Some(p) => {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                    self.bookmark_display_path(c.is_ascii_lowercase(), &p)
                ))
            }
            None => {
                self.flash = Some(format!(
                    "{} '{c}'",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::NoBookmark)
                ))
            }
        }
    }
    /// Make the bookmark target (a directory) the new root and show the Tree. Does not change open_dir (the local key).
    pub(super) fn jump_to_dir(&mut self, dir: std::path::PathBuf) {
        // Since root is changing, discard the old root's selection/visual/filter/search state
        // (carrying them over is a footgun: the old root's files could become targets of a
        // mistaken operation while the markers stay invisible).
        self.clear_for_root_change();
        self.tab.root = dir;
        self.tab.entries.clear();
        self.tab.selected = 0;
        self.rebuild_tree_notify();
        self.tab.mode = Mode::Tree;
        self.tab.preview_path = None;
        self.tab.preview_kind = None;
    }

    /// `:`=make the current location the "anchored root" (**no text input**). Re-anchors the current tree root,
    /// navigated to with `h`/`l`, to the display base (open_dir), so relative-path display, the `yr` path copy, and
    /// the title all become relative to the current location (resolving what used to show as `../` from launch). Leaves the tree structure and cursor unchanged.
    pub fn reanchor_root(&mut self) {
        if self.tab.open_dir == self.tab.root {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::AlreadyRoot).into());
            return;
        }
        self.tab.open_dir = self.tab.root.clone();
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::Root),
            home_relative(&self.tab.root)
        ));
    }

    /// `A`=reset the anchor (display base = open_dir) back to the **launch dir** (the counterpart of reanchor_root=`a`).
    /// Resets the anchor that `a` moved to the current location back to the startup position, restoring relative-path display.
    /// Leaves the tree structure / cursor / root unchanged (moves only open_dir, symmetric to reanchor_root).
    pub fn reset_anchor(&mut self) {
        if self.tab.open_dir == self.launch_dir {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::AlreadyAtStartDir).into());
            return;
        }
        self.tab.open_dir = self.launch_dir.clone();
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::AnchorReset),
            home_relative(&self.launch_dir)
        ));
    }

    // --- Bookmark list overlay ----------------------------------------
    pub fn is_bookmark_list(&self) -> bool {
        self.bookmark_list
    }
    pub fn open_bookmark_list(&mut self) {
        self.bookmark_list = true;
        self.bookmark_list_sel = 0;
    }
    pub fn close_bookmark_list(&mut self) {
        self.bookmark_list = false;
    }
    pub fn bookmark_list_sel(&self) -> usize {
        self.bookmark_list_sel
    }
    /// List items (is_local, key, path). Local first, then global.
    pub fn bookmark_list_items(&self) -> Vec<(bool, char, std::path::PathBuf)> {
        let (local, global) = self.bookmarks.list();
        let mut v = Vec::with_capacity(local.len() + global.len());
        for (k, p) in local {
            v.push((true, k, p));
        }
        for (k, p) in global {
            v.push((false, k, p));
        }
        v
    }
    pub fn bookmark_list_move(&mut self, delta: i32) {
        let n = self.bookmark_list_items().len();
        if n == 0 {
            return;
        }
        self.bookmark_list_sel =
            (self.bookmark_list_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }
    /// Jump to the selected bookmark (closes the list).
    pub fn bookmark_list_jump(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((is_local, _, p)) = items.get(self.bookmark_list_sel).cloned() {
            self.bookmark_list = false;
            if p.is_dir() {
                self.jump_to_dir(p);
            } else if p.is_file() {
                self.enter_preview(&p); // a file opens in the preview (tree is left unchanged)
            } else {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                    self.bookmark_display_path(is_local, &p)
                ));
            }
        }
    }
    /// If the selected list item is a file, **open it directly in the editor** without going through the preview
    /// (closes the list and sets pending_edit; the run loop launches run_editor). Directories cannot be edited and flash.
    pub fn bookmark_list_edit(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((is_local, _, p)) = items.get(self.bookmark_list_sel).cloned() {
            if p.is_file() {
                self.bookmark_list = false;
                self.pending_edit = Some((p, None));
            } else if p.is_dir() {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::CannotEditDirectory).into());
            } else {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                    self.bookmark_display_path(is_local, &p)
                ));
            }
        }
    }
    /// Delete the selected bookmark (the list stays open; the selection is clamped).
    pub fn bookmark_list_delete(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((_, key, _)) = items.get(self.bookmark_list_sel) {
            let key = *key;
            if let Err(e) = self.bookmarks.remove(key) {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
            }
            let n = self.bookmark_list_items().len();
            if self.bookmark_list_sel >= n {
                self.bookmark_list_sel = n.saturating_sub(1);
            }
        }
    }

    /// Start filtering (`/`). Recursively collects everything under root into a pool and enters input mode.
    pub fn start_filter(&mut self) {
        self.tab.filter_pool = collect_all(&self.tab.root, self.tab.show_hidden);
        self.tab.filter_input = Some(String::new());
        self.tab.tree_filter = Some(String::new());
        self.tab.selected = 0;
        self.reapply_filter();
    }

    /// Add one character to the filter input (live filtering).
    pub fn filter_input_push(&mut self, c: char) {
        if let Some(q) = self.tab.filter_input.as_mut() {
            q.push(c);
        }
        self.tab.tree_filter = self.tab.filter_input.clone();
        self.reapply_filter();
    }

    /// Delete one character from the filter input.
    pub fn filter_input_backspace(&mut self) {
        if let Some(q) = self.tab.filter_input.as_mut() {
            q.pop();
        }
        self.tab.tree_filter = self.tab.filter_input.clone();
        self.reapply_filter();
    }

    /// Commit input (Enter): leave editing but keep the filtered results (navigate normally afterwards).
    pub fn filter_commit(&mut self) {
        self.tab.filter_input = None;
    }

    /// Clear the filter (Esc): return to the normal tree.
    pub fn filter_clear(&mut self) {
        self.clear_filter_state();
        self.tab.selected = 0;
        self.rebuild_tree_notify();
    }

    /// Discard the filter state (input/query/pool and the changed-files filter) (does not rebuild the tree).
    pub(super) fn clear_filter_state(&mut self) {
        self.tab.filter_input = None;
        self.tab.tree_filter = None;
        self.tab.filter_pool = Vec::new();
        self.tab.changed_filter = false;
    }

    /// Whether input mode (key interception) is active.
    pub fn is_filtering(&self) -> bool {
        self.tab.filter_input.is_some()
    }

    /// The query while a filter is applied (including after the input is committed). Used for title / relative-path display.
    pub fn filter_query(&self) -> Option<&str> {
        self.tab.tree_filter.as_deref()
    }

    /// Filter the pool by the current query to build entries.
    /// `[ui] filter_mode` (see `UiConfig::filter_mode`): `"fuzzy"` (default) = ranked fuzzy
    /// subsequence match via `fuzzy_filter_pool`; `"substring"` = the legacy plain
    /// case-insensitive substring match, kept in the pool's original order (no ranking).
    /// When the query is empty (= right after pressing `/`, before typing any character), **show nothing**
    /// (avoiding a flat display of everything, which would look like "expand all").
    fn reapply_filter(&mut self) {
        let q = self.tab.tree_filter.clone().unwrap_or_default();
        self.tab.entries = if q.is_empty() {
            Vec::new()
        } else if self.cfg.ui.filter_mode() == "substring" {
            let q = q.to_lowercase();
            self.tab
                .filter_pool
                .iter()
                .filter(|e| {
                    e.path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase().contains(&q))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        } else {
            fuzzy_filter_pool(&self.tab.filter_pool, &q)
        };
        if self.tab.selected >= self.tab.entries.len() {
            self.tab.selected = self.tab.entries.len().saturating_sub(1);
        }
        // The filtered result is a different entries set, so visual_anchor (an index) is stale. Invalidate it.
        self.tab.visual_anchor = None;
    }

    // --- Changed-files-only filter (`C`) + jump between changes (`n`/`N`) — Agent Watch ① ----------
    /// Whether the changed-files-only tree filter is active (drives the flat relative-path rendering).
    pub fn changed_filter(&self) -> bool {
        self.tab.changed_filter
    }

    /// The sorted list of files with a git status under the current root (the review work-list).
    /// Deleted files have a status but no longer exist, so they are excluded (nothing to preview/select).
    pub(super) fn changed_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .git_status
            .keys()
            .filter(|p| p.starts_with(&self.tab.root) && p.is_file())
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// `C`: toggle the changed-files-only view. ON shows the flat list of changed files (like the `/`
    /// filter's result list — review them top to bottom); OFF returns to the normal tree. When there is
    /// no change, stays on the tree with a flash (never trap the user in an empty list).
    pub fn toggle_changed_filter(&mut self) {
        if self.tab.changed_filter {
            self.tab.changed_filter = false;
            self.tab.selected = 0;
            self.rebuild_tree_notify();
            return;
        }
        self.ensure_git_status_now(); // `C` answers from status = if a scan is in flight, wait (never show a false "no changes")
        if self.changed_paths().is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        self.clear_filter_state(); // mutually exclusive with the name filter (`/`)
        self.tab.changed_filter = true;
        self.tab.selected = 0;
        self.reapply_changed_filter();
    }

    /// Rebuild entries as the flat changed-files list. Called on toggle and by `refresh_fs` while active
    /// (an agent committing/reverting updates the list live). Auto-exits to the tree when the last change
    /// disappears (e.g. everything was committed) instead of leaving an empty list.
    pub(super) fn reapply_changed_filter(&mut self) {
        let entries: Vec<super::Entry> = self
            .changed_paths()
            .into_iter()
            .map(|path| super::Entry {
                path,
                is_dir: false,
                depth: 0,
                expanded: false,
            })
            .collect();
        if entries.is_empty() {
            self.tab.changed_filter = false;
            self.tab.selected = 0;
            self.rebuild_tree_notify();
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        self.tab.entries = entries;
        if self.tab.selected >= self.tab.entries.len() {
            self.tab.selected = self.tab.entries.len().saturating_sub(1);
        }
        self.tab.visual_anchor = None;
    }

    /// `n`/`N` on the tree: jump the cursor to the next/previous changed file in path order (wraps).
    /// In the normal tree the target is revealed (collapsed ancestors expanded); in the changed-only
    /// filter it moves within the flat list.
    pub fn jump_changed(&mut self, dir: i64) {
        // While a diff view is shown, "switch to the next/previous changed file's diff" cycles
        // through changes without leaving the view (a hunk/lazygit-style review flow). In the tree it's
        // still the usual cursor jump.
        if self.is_git_diff_preview() {
            self.diff_jump_changed(dir);
            return;
        }
        self.ensure_git_status_now(); // `n`/`N` also answer from status
        let paths = self.changed_paths();
        if paths.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        let len = paths.len() as i64;
        let idx = match self
            .tab
            .entries
            .get(self.tab.selected)
            .map(|e| e.path.clone())
        {
            Some(cur) => match paths.binary_search(&cur) {
                Ok(i) => (i as i64 + dir).rem_euclid(len) as usize,
                // The cursor is not on a changed file: go to the next (previous) insertion point (wrapping).
                Err(ins) => {
                    if dir > 0 {
                        (ins as i64).rem_euclid(len) as usize
                    } else {
                        (ins as i64 - 1).rem_euclid(len) as usize
                    }
                }
            },
            None => 0,
        };
        let target = paths[idx].clone();
        if self.tab.changed_filter {
            if let Some(i) = self.tab.entries.iter().position(|e| e.path == target) {
                self.tab.selected = i;
            }
            return;
        }
        match self.reveal_path_deep(&target) {
            Ok(true) => {}
            // The target can't be made visible, e.g. it's under a hidden directory (switching to show hidden with `.` would reach it).
            Ok(false) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::JumpTargetHidden).into());
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
            }
        }
    }

    /// `n`/`N` inside the GitDiff preview: switch the diff target to the next/previous changed file
    /// (wraps) without leaving the view. **Scope depends on how the diff was opened**: a follow-opened
    /// diff cycles only the files changed during the current follow session ("what just changed"),
    /// while a tree-/hub-opened diff cycles the full uncommitted change set. The tree cursor follows
    /// (so `q` lands on the file just reviewed), and where `q` returns to (hub vs tree) is preserved.
    fn diff_jump_changed(&mut self, dir: i64) {
        let paths = if self.diff_follow_scope {
            self.follow_session_paths()
        } else {
            self.ensure_git_status_now();
            self.changed_paths()
        };
        if paths.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        let Some(crate::preview::PreviewKind::GitDiff(cur)) = self.tab.preview_kind.clone() else {
            return;
        };
        let len = paths.len() as i64;
        let idx = match paths.iter().position(|p| *p == cur) {
            Some(i) => (i as i64 + dir).rem_euclid(len) as usize,
            // The currently shown file isn't in the set (e.g. it was just committed): start from the first/last.
            None => {
                if dir > 0 {
                    0
                } else {
                    (len - 1) as usize
                }
            }
        };
        let target = paths[idx].clone();
        if target == cur {
            return; // only one target
        }
        // Sync the tree cursor too (so returning with `q` lands on the file that was just being viewed).
        if self.tab.changed_filter {
            self.reapply_changed_filter();
            if let Some(i) = self.tab.entries.iter().position(|e| e.path == target) {
                self.tab.selected = i;
            }
        } else {
            let _ = self.reveal_path_deep(&target);
        }
        // open_git_diff resets came_from_git_view / diff_follow_scope, so save and restore them
        // (so cycling from a hub-opened diff still returns to the hub with `q`, and a follow-scoped cycle keeps going).
        let came = self.tab.came_from_git_view;
        let scope = self.diff_follow_scope;
        self.open_git_diff(&target);
        self.tab.came_from_git_view = came;
        self.diff_follow_scope = scope;
    }

    /// The follow session's reviewable files (recorded while `F` was ON), pruned to still-existing
    /// non-media files. First-change order (chronological — the review order of "what just happened").
    fn follow_session_paths(&self) -> Vec<PathBuf> {
        self.follow_session
            .iter()
            .filter(|p| p.is_file() && !self.follow_is_media(p))
            .cloned()
            .collect()
    }

    /// The current GitDiff target's position within its navigation set (1-based, total) — the follow
    /// session for follow-opened diffs, the full change set otherwise. For the title's `(2/5)` indicator.
    pub fn diff_change_position(&self) -> Option<(usize, usize)> {
        let crate::preview::PreviewKind::GitDiff(p) = self.tab.preview_kind.as_ref()? else {
            return None;
        };
        let paths = if self.diff_follow_scope {
            self.follow_session_paths()
        } else {
            self.changed_paths()
        };
        let i = paths.iter().position(|q| q == p)?;
        Some((i + 1, paths.len()))
    }

    /// Refetch git status if root changed (called just before rendering). Does nothing if root is unchanged.
    /// As a result, expand/collapse does not refetch; updates happen only on directory moves and tab switches.
    ///
    /// The cheap `statuses`+`branch` (tens of ms, within the <60ms budget) is refetched synchronously, but the
    /// **heavy `ignored` (~800ms on large repos) is offloaded to a separate thread** (the "don't block the UI" principle). Rendering stays responsive, and
    /// only the dimming based on ignore rules appears after the result arrives (`apply_ignored`).
    pub fn refresh_git_if_needed(&mut self) {
        // Do nothing if root is the same and not dirty. dirty (a re-validation request) refetches even for the same root.
        if self.git_status_for.as_deref() == Some(self.tab.root.as_path()) && !self.git_status_dirty
        {
            return;
        }
        let wd = crate::git::workdir(&self.tab.root);
        // statuses/branch run `git status` **from the workdir**: within the same repository the
        // result is identical from any subdirectory. So **when workdir is the same and not dirty,
        // reuse it instead of recomputing** (this avoids synchronously running a whole-worktree-scanning
        // `git status` on every `l`/`h` descent = the main reason h/l felt heavy in a large repo).
        // Only refetch when dirty (file change / commit / checkout / ignore-rule change) or when moving to a different repo.
        // Mirrors `ignored`'s Phase G (per-workdir cache).
        let status_reusable = self.git_status_workdir.is_some()
            && self.git_status_workdir == wd
            && !self.git_status_dirty;
        if status_reusable {
            // Same repo, no re-validation request: reuse the existing status as-is (an optimization for `l`/`h` descent).
            self.git_status_for = Some(self.tab.root.clone());
        } else {
            // Offload the full `git status` (a whole-worktree scan) **to a separate thread**. The
            // previous status keeps showing until it arrives, so the tree draws instantly
            // (principle "don't block the UI").
            self.kick_status_refresh(wd.clone());
        }
        self.refresh_ignored_if_needed(wd);
    }

    /// The `ignored` half of `refresh_git_if_needed`, split out so the synchronous
    /// `ensure_git_status_now` can reuse it without also going through the async status dispatch.
    fn refresh_ignored_if_needed(&mut self, wd: Option<PathBuf>) {
        // The heavy `ignored` (ignore set) is only rebuilt **when repo (workdir) changes**. Descending
        // into a subdirectory within the same repository keeps the same ignore rules, so it's reused
        // (avoids recomputation on `l` descent).
        let different_repo = self.git_ignored_for != wd;
        let need = self.git_ignored_dirty || different_repo;
        // If a computation for the same workdir is already in flight, wait (when ignore-rule-change
        // dirty is set, a newer result than that is needed).
        // The `is_some()` check is required: for a root that isn't a repo, both `wd` and
        // `git_ignored_pending` are None, so without it "in progress" would **always hold**, causing
        // an early return that never reached the None branch below (which discards the previous
        // repo's ignore set). A computation only ever runs against a real workdir.
        let inflight = self.git_ignored_pending.is_some()
            && self.git_ignored_pending == wd
            && !self.git_ignored_dirty;
        if !need || inflight {
            return;
        }
        match wd {
            None => {
                // Not a repo: clear immediately (no computation needed).
                self.git_ignored.clear();
                self.git_ignored_for = None;
                self.git_ignored_pending = None;
                self.git_ignored_dirty = false;
            }
            Some(wd) => {
                // Only clear the old set when moving to a different repo (avoids a mix of dimmed
                // states). When rebuilding the same repo due to an ignore-rule change (dirty), keep
                // showing the old set until the new one arrives (avoids flicker).
                if different_repo {
                    self.git_ignored.clear();
                    self.git_ignored_for = None;
                }
                self.git_ignored_gen = self.git_ignored_gen.wrapping_add(1);
                self.git_ignored_pending = Some(wd.clone());
                self.git_ignored_dirty = false;
                let gen = self.git_ignored_gen;
                self.spawn_or_sync_ignored(self.tab.root.clone(), wd, gen);
            }
        }
    }

    /// Attach the Sender of the worker that computes `ignored` in the background (called by main at startup).
    pub fn attach_git_loader(&mut self, tx: std::sync::mpsc::Sender<IgnoredResult>) {
        self.ignored_tx = Some(tx);
    }

    /// Attach the Sender of the worker that computes `statuses`+`branch` in the background (called by main at startup).
    pub fn attach_status_loader(&mut self, tx: std::sync::mpsc::Sender<crate::app::StatusResult>) {
        self.status_tx = Some(tx);
    }

    /// Request a `git status` refresh for the current root, **without blocking**.
    ///
    /// Within the same repository the previous statuses are deliberately **kept on screen until the new
    /// ones arrive**: entries are keyed by absolute path, so nothing shifts under the cursor and the
    /// markers don't blink off on every root/tab change. Moving to a **different repo** does clear them,
    /// because `git_branch` (the tree title) and `git_has_changes()` (the gutter column) are *not*
    /// path-keyed and would otherwise show the previous repository's branch for the length of a scan.
    fn kick_status_refresh(&mut self, wd: Option<PathBuf>) {
        // If a scan for the same root is already in flight, **don't spawn a new one**. Keep the
        // re-validation request (dirty) set as-is, and pick it up exactly once when the result
        // arrives (`apply_statuses`) to re-run = **request coalescing**.
        // Back when this ran synchronously, the scan's own duration naturally rate-limited the next
        // request, but that rate-limiting disappears once it's async, so without coalescing here a
        // whole-worktree scan would pile up on every fs event (worse the bigger the repo an agent
        // keeps writing to = exactly the situation this fix targets).
        if self.git_status_pending.as_deref() == Some(self.tab.root.as_path()) {
            return;
        }
        // When moving to a different repo, discard the non-path-keyed derived displays (branch name /
        // whether the gutter column shows) so they don't keep showing the previous repo's. This path
        // isn't taken within the same repo (`l`/`h`), so there's no flicker there.
        if self.git_status_workdir != wd {
            self.git_status.clear();
            self.git_branch = None;
            self.git_status_workdir = None;
        }
        self.git_status_gen = self.git_status_gen.wrapping_add(1);
        self.git_status_pending = Some(self.tab.root.clone());
        // This generation's computation has taken over the request. Any further re-validation requests are picked up in the next generation.
        self.git_status_dirty = false;
        // A marker meaning "we have / are fetching status for this root". Without setting this ahead
        // of time, we'd re-kick on every render until the result arrives (double protection along with the inflight guard above).
        self.git_status_for = Some(self.tab.root.clone());
        let gen = self.git_status_gen;
        let root = self.tab.root.clone();
        self.spawn_or_sync_statuses(root, wd, gen);
    }

    /// Compute `statuses`+`branch` on a separate thread and return them via the Sender. With no Sender attached
    /// (tests / no channel), fall back to **synchronous** computation and application, so unit tests that don't
    /// drive a run loop still observe the result immediately (same contract as `spawn_or_sync_ignored`).
    fn spawn_or_sync_statuses(&mut self, root: PathBuf, workdir: Option<PathBuf>, gen: u64) {
        // If it's not a repo (including a no-git build), the result is trivially empty. Settle it immediately without spawning a thread.
        if workdir.is_none() {
            let res = crate::app::StatusResult {
                gen,
                statuses: Default::default(),
                branch: None,
                workdir,
            };
            self.apply_statuses(res);
            return;
        }
        let Some(tx) = self.status_tx.clone() else {
            let res = Self::scan_statuses(root, workdir, gen);
            self.apply_statuses(res);
            return;
        };
        std::thread::spawn(move || {
            // If the worker panics, no result comes back and `git_status_pending` stays set forever,
            // spinning the spinner and freezing status. Always return a result, with the same safety net as the other workers (principle #3).
            if let Some(res) =
                crate::preview::markdown::catch_silent(|| Self::scan_statuses(root, workdir, gen))
            {
                let _ = tx.send(res);
            }
        });
    }

    /// The actual scan (pure: no `&self`), shared by the worker thread and the synchronous fallbacks.
    fn scan_statuses(
        root: PathBuf,
        workdir: Option<PathBuf>,
        gen: u64,
    ) -> crate::app::StatusResult {
        crate::app::StatusResult {
            gen,
            statuses: crate::git::statuses(&root),
            branch: crate::git::branch(&root),
            workdir,
        }
    }

    /// Guarantee `git_status`/`git_branch` describe the working tree **right now**, computing synchronously
    /// if a background scan is still in flight.
    ///
    /// The two paths have deliberately different contracts:
    /// - `refresh_git_if_needed` = the **render path**. Never blocks; a stale frame is fine because the
    ///   result lands a moment later and repaints.
    /// - this = an **explicit user command** whose answer is derived from the status (`d` open diff,
    ///   `C` changed-only filter, `n`/`N` jump to change, the branch label right after a checkout).
    ///   These must not answer "no changes" just because a scan hasn't finished — that reads as a broken
    ///   feature rather than a slow one.
    pub fn ensure_git_status_now(&mut self) {
        let wd = crate::git::workdir(&self.tab.root);
        // Do nothing if not in flight and we already have the latest for the same repo (the same judgment as `l`/`h`'s reuse).
        let fresh = self.git_status_pending.is_none()
            && self.git_status_workdir.is_some()
            && self.git_status_workdir == wd
            && !self.git_status_dirty;
        if fresh {
            self.git_status_for = Some(self.tab.root.clone());
        } else {
            // We deliberately **don't dispatch to a separate thread here**. Dispatching and then also
            // computing synchronously would run one extra full scan just to be discarded (more waste
            // the bigger the repo). By bumping the generation, any scan already in flight will have
            // its result discarded even if it arrives.
            self.git_status_gen = self.git_status_gen.wrapping_add(1);
            self.git_status_dirty = false;
            self.git_status_pending = None;
            let res = Self::scan_statuses(self.tab.root.clone(), wd.clone(), self.git_status_gen);
            self.apply_statuses(res);
        }
        self.refresh_ignored_if_needed(wd);
    }

    /// Apply a `statuses` result from the other thread. Discards results whose generation is stale (the user
    /// moved to another root/tab while it was computing). Returns true if state changed (the caller redraws).
    pub fn apply_statuses(&mut self, res: crate::app::StatusResult) -> bool {
        if res.gen != self.git_status_gen {
            return false; // stale: already moved to a different root/tab
        }
        self.git_status = res.statuses;
        self.git_branch = res.branch;
        self.git_status_workdir = res.workdir;
        // A matching generation = this result belongs to the current workdir. The root at the time
        // the scan started can differ from the current root if `l`/`h` moved within the same repo.
        // Recording that stale value would make `refresh_git_status_only` return early on the next fs
        // event and **completely miss a re-validation**, so record the current root instead.
        self.git_status_for = Some(self.tab.root.clone());
        self.git_status_pending = None;
        // Run the re-validation request that arrived while computing (the coalesced one) exactly once here.
        if self.git_status_dirty {
            let wd = crate::git::workdir(&self.tab.root);
            self.kick_status_refresh(wd);
        }
        // The "changed files only" filter's list is entries **already derived** from statuses, so a
        // re-render alone won't fix it. Rebuild it the moment the new status arrives (live follow-along for an agent's edits).
        if self.tab.changed_filter {
            self.reapply_changed_filter();
        }
        true
    }

    /// Compute the heavy `git::ignored(root)` on a separate thread and return the result via the Sender. If no Sender is attached (tests /
    /// no channel), fall back to **synchronous** immediate computation and application (keeping unit tests that don't assume rendering working).
    fn spawn_or_sync_ignored(&mut self, root: PathBuf, workdir: PathBuf, gen: u64) {
        let Some(tx) = self.ignored_tx.clone() else {
            // Synchronous fallback: compute and apply it on the spot (no staleness).
            let set = crate::git::ignored(&root);
            self.apply_ignored(IgnoredResult { gen, workdir, set });
            return;
        };
        std::thread::spawn(move || {
            let set = crate::git::ignored(&root);
            let _ = tx.send(IgnoredResult { gen, workdir, set });
        });
    }

    /// Apply the `ignored` computation result from the other thread. Discards results with an old generation (moved to a different repo during computation).
    /// Returns true if applying changed the state (the caller redraws).
    pub fn apply_ignored(&mut self, res: IgnoredResult) -> bool {
        if res.gen != self.git_ignored_gen {
            return false; // stale: already moved to a different repo / generation
        }
        self.git_ignored = res.set;
        self.git_ignored_for = Some(res.workdir);
        self.git_ignored_pending = None;
        true
    }

    /// Returns the git status of a given path.
    pub fn git_status_of(&self, path: &Path) -> Option<crate::git::FileStatus> {
        self.git_status.get(path).copied()
    }

    /// Whether `path` is excluded by gitignore (itself or an **ancestor directory** is in the ignored set).
    /// Even when expanding under `node_modules` (= one entry), an ancestor match lets the contents be dimmed too. Does not look above root.
    pub fn is_ignored(&self, path: &Path) -> bool {
        if self.git_ignored.is_empty() {
            return false;
        }
        let mut p = path;
        loop {
            if self.git_ignored.contains(p) {
                return true;
            }
            if p == self.tab.root {
                return false;
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => return false,
            }
        }
    }

    /// Whether an fs-event burst is pure build churn — every changed path is gitignored and no ignore
    /// rule changed — so the tree/status refresh can be skipped. An empty `changed` (unknown / `.git`-only)
    /// is NOT churn (returns false) so external git operations still refresh. Follow is unaffected: its
    /// candidates are already filtered by `is_ignored` in `follow_note_change`.
    pub(crate) fn fs_burst_is_build_churn(
        &self,
        changed: &[std::path::PathBuf],
        ignore_rules_changed: bool,
    ) -> bool {
        !changed.is_empty() && !ignore_rules_changed && changed.iter().all(|p| self.is_ignored(p))
    }

    /// The current branch name (None if not a repo). Used for the tree's title display.
    pub fn git_branch(&self) -> Option<&str> {
        self.git_branch.as_deref()
    }

    /// Whether there is at least one change (= a git repo with uncommitted changes). Used to decide the status-gutter display.
    pub fn git_has_changes(&self) -> bool {
        !self.git_status.is_empty()
    }

    /// Reload: refetch the directory listing and git status (`r` key, FSEvents, returning from an external tool).
    /// Keeps derived state in sync **in one place** to prevent the display from diverging from reality:
    /// - **Prune selection to existing paths only** (prevents one externally-deleted item from making a batch trash operation fail entirely #12).
    ///   A broken symlink itself is kept as existing (judged via `symlink_metadata` without following → don't wrongly drop it from operation targets).
    /// - **Recompute only the active derived views** (don't touch inactive ones, to avoid over-recomputation;
    ///   same spirit as the root guard in `refresh_git_if_needed`): in the Git view, the change list; in Preview mode,
    ///   refetch the current preview (reflecting git_view_entries staleness on return from an external git tool #4 / preview staleness on external edit).
    pub fn refresh(&mut self) -> Result<()> {
        // An explicit reload (`r` / fileops / returning from an external tool) also rebuilds the ignore set (full recompute).
        self.refresh_fs(true)
    }

    /// Reload from FSEvents. When `recompute_ignored=false`, **skip recomputing the heavy ignore set (`ignored`)**
    /// and refetch only the cheap `statuses`+`branch`. Ignore rules (`.gitignore` /
    /// `.git/info/exclude`) rarely change, so this is called with `true` only when they actually change, also rebuilding
    /// `ignored`. This avoids a few-hundred-ms freeze per event on large repositories.
    pub fn refresh_fs(&mut self, recompute_ignored: bool) -> Result<()> {
        // Unknown changed paths = the safe side (also reload the preview).
        self.refresh_fs_changed(recompute_ignored, &[])
    }

    /// The **changed-paths-aware** version of `refresh_fs`. When `changed` is non-empty, reload the
    /// current preview only when it's actually affected by that change (see `preview_affected_by`).
    ///
    /// Purpose: while an agent keeps rewriting `src/`, avoid rebuilding the decoration of an
    /// unrelated `docs/foo.md` on every single event (= re-rendering the whole Markdown / re-parsing
    /// the entire CSV). Tree rebuild, git status, the changed filter, and the git view still follow along on every event as before.
    pub fn refresh_fs_changed(
        &mut self,
        recompute_ignored: bool,
        changed: &[std::path::PathBuf],
    ) -> Result<()> {
        self.refresh_fs_inner(recompute_ignored, changed, true)
    }

    /// The tab-switch variant: refresh the tree / git status / derived views, but **do not reload the
    /// preview**. `load_active` has just rebuilt it from disk itself (`setup_windowed` reopens the file,
    /// `load_table` re-parses it, `md_cache = None` makes the next draw re-read the source, and
    /// `start_media_load` re-reads the media), so going through `reload_preview` here only repeated the
    /// same work — a second full CSV parse for a table tab, a second window open for a text tab.
    pub(super) fn refresh_fs_after_tab_switch(&mut self) -> Result<()> {
        self.refresh_fs_inner(false, &[], false)
    }

    fn refresh_fs_inner(
        &mut self,
        recompute_ignored: bool,
        changed: &[std::path::PathBuf],
        reload_preview: bool,
    ) -> Result<()> {
        if recompute_ignored {
            self.git_status_for = None; // recompute statuses+branch on the next render
            self.git_status_dirty = true; // an ignore-rule change can also change status output = invalidate the workdir cache
            self.git_ignored_dirty = true; // rebuild the heavy ignored too (ignore-rule change / explicit refresh).
                                           // The old set is kept until the new one lands (computed on a separate thread, avoids flicker).
        } else {
            self.refresh_git_status_only(); // statuses+branch only (ignored keeps its cache)
        }
        self.diff_cache = None; // the working tree may have changed → drop the diff cache (keeps up with external edits)
        self.gutter_cache = None; // same as above: the git change gutter is also rebuilt on a working-tree change
                                  // We do rebuild the tree, but its transient failure (e.g. a subdirectory being
                                  // expanded briefly becomes unreadable while an agent bulk-rewrites files)
                                  // must **not skip** the preview/view reload below. Those only depend on
                                  // preview_path / git state, not the new tree, so the tree's Err is returned at the end.
        let tree = self.rebuild_tree();
        // While the changed-files-only filter is active, rebuild the list from the latest statuses (follows an agent's edits).
        if self.tab.changed_filter {
            self.refresh_git_if_needed();
            // Must not rebuild while a scan is in flight. Right after switching back from a
            // different repo's tab, `git_status` is empty (right after the kick discarded the other
            // repo's leftovers), so rebuilding here would make the filter judge "zero changes" and
            // **silently turn itself off, even showing a false "no changed files"**. Wait instead —
            // `apply_statuses` rebuilds it once the result arrives.
            if self.git_status_pending.is_none() {
                self.reapply_changed_filter();
            }
        } else if self.tab.tree_filter.is_some() {
            // While the text filter (`/`) is active, rebuild_tree resets entries back to showing
            // everything. Re-collect the pool from the current tree (follows external
            // additions/deletions), then re-filter with the saved/restored query (fixing the
            // inconsistency where the query is restored but the list shows everything).
            self.tab.filter_pool = collect_all(&self.tab.root, self.tab.show_hidden);
            self.reapply_filter();
        }
        // Drop paths that vanished from the selection set (retain keeps only ones that still exist; symlinks aren't followed).
        self.tab.selection.retain(|p| p.symlink_metadata().is_ok());
        // Only make the active derived view follow along.
        if self.is_git_view() {
            self.git_view_reload();
        }
        if reload_preview
            && matches!(self.tab.mode, Mode::Preview)
            && self.preview_affected_by(changed)
        {
            self.reload_preview();
        }
        tree
    }

    /// Whether the current preview must be re-read because of this FS event.
    ///
    /// `changed` carries the non-`.git` paths from the event (the watcher strips repository
    /// internals). **An empty slice therefore means "unknown or `.git`-only"** — a commit, a
    /// checkout, or an event we could not attribute — and always reloads, so git diff previews and
    /// change gutters never go stale.
    ///
    /// Otherwise only the previewed file itself matters. Inline Markdown images are deliberately not
    /// considered: `md_image_cache` is keyed by path and only cleared when switching files, so those
    /// are not re-decoded by a reload either — gating here does not make them any staler.
    fn preview_affected_by(&self, changed: &[std::path::PathBuf]) -> bool {
        if changed.is_empty() {
            return true;
        }
        // A git diff display is a function of "file + git state". Even if a `.git` change is mixed
        // into the same event (= it doesn't appear here), always follow along to avoid missing it.
        if self.is_git_diff_preview() {
            return true;
        }
        let Some(cur) = self.tab.preview_path.as_deref() else {
            return true;
        };
        changed.iter().any(|p| p == cur)
    }

    /// Cheap git update: refetch only `statuses` + `branch`, **leaving the heavy `ignored` (ignore set) untouched**
    /// (keeping the cache). Does nothing if the initial `ignored` computation hasn't happened yet (`git_status_for` unset)
    /// (the next render's `refresh_git_if_needed` computes everything).
    fn refresh_git_status_only(&mut self) {
        if self.git_status_for.as_deref() != Some(self.tab.root.as_path()) {
            return;
        }
        // Refetching goes to **a separate thread**. This is called from within `handle_key` (fs
        // events / tab switches), so running it synchronously froze key input itself for the
        // duration of `git status`.
        let wd = crate::git::workdir(&self.tab.root);
        self.git_status_dirty = true; // refetch even for the same workdir (a re-validation request)
        self.kick_status_refresh(wd);
    }
}

// --- Fuzzy matching for tree filtering (`/`) -----------------------------------------------------

thread_local! {
    // `nucleo_matcher::Matcher::new` eagerly allocates ~135KB, so nucleo's own docs recommend
    // reusing one instance rather than building it per call. konoma's run loop is single-threaded,
    // so a thread-local scratch instance is reused across every keystroke of every filter session
    // without needing a dedicated `App`/`PerTab` field.
    static FUZZY_MATCHER: std::cell::RefCell<nucleo_matcher::Matcher> =
        std::cell::RefCell::new(nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT));
}

/// fzf-style fuzzy-match `query` against each entry's file/directory name (the leaf name only —
/// the same target the legacy substring mode uses), keep only entries that match, and return them
/// ranked best-score-first. Ties keep the pool's original relative order (`Vec::sort_by` is a
/// stable sort in Rust, so no explicit tie-break is needed).
///
/// Matching is always case-insensitive (`CaseMatching::Ignore`), matching the legacy substring
/// mode's behavior — a query typed in any case still finds everything. (nucleo also offers
/// `CaseMatching::Smart`, the fzf/ripgrep convention where an all-uppercase query becomes
/// case-sensitive; that would be a user-visible behavior change from konoma's historical
/// always-case-insensitive filter, so it isn't the default here.) Space-separated words in the
/// query are AND-ed (each must match somewhere), fzf-style — this and the `^`/`$`/`'`/`!` special
/// atom syntax come for free from `Pattern::parse`.
pub(super) fn fuzzy_filter_pool(pool: &[Entry], query: &str) -> Vec<Entry> {
    use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    FUZZY_MATCHER.with(|cell| {
        let mut matcher = cell.borrow_mut();
        let mut buf: Vec<char> = Vec::new();
        let mut scored: Vec<(u32, usize)> = pool
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let name = e.path.file_name().and_then(|n| n.to_str())?;
                buf.clear();
                let haystack = nucleo_matcher::Utf32Str::new(name, &mut buf);
                pattern
                    .score(haystack, &mut matcher)
                    .map(|score| (score, i))
            })
            .collect();
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        scored.into_iter().map(|(_, i)| pool[i].clone()).collect()
    })
}
