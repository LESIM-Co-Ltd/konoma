use super::*;

impl App {
    // --- Follow mode (`F`) — Agent Watch ② -------------------------------------
    /// `F`: toggle follow mode (auto-jump to externally changed files; the "watch the AI work" view).
    /// Turning it ON starts a fresh follow session (the "what changed while following" list for `n`/`N`).
    pub fn toggle_follow(&mut self) {
        self.follow_mode = !self.follow_mode;
        if self.follow_mode {
            self.follow_session.clear();
            // A new follow session: default to "since start" display, pinning this moment as the baseline.
            self.follow_diff_full = false;
            // If a follow diff is currently displayed, re-fetch it against the new baseline (drop the stale old diff a matching path would otherwise keep).
            self.diff_cache = None;
            #[cfg(feature = "git")]
            self.capture_follow_baseline();
            // Pin the root this session/baseline describes (see `follow_root`'s doc comment).
            self.follow_root = Some(self.tab.root.clone());
        }
        // On OFF, keep the baseline (and follow_root) so that after follow_break/F-off, `n`/`N`
        // cycling and revisiting via the `f` toggle still work on top of diff_follow_scope; it's
        // rebuilt on the next F — bounded memory).
        let msg = if self.follow_mode {
            crate::i18n::Msg::FollowOn
        } else {
            crate::i18n::Msg::FollowOff
        };
        self.flash = Some(tr(self.lang, msg).into());
    }

    /// Capture the follow-start snapshot: the content of every currently-dirty file (bounded by size),
    /// plus the pinned HEAD sha for clean files. Dirty files too large to snapshot are recorded as
    /// `None` so their follow diff falls back to the full git diff. `F` is a command → sync status first.
    #[cfg(feature = "git")]
    fn capture_follow_baseline(&mut self) {
        self.ensure_git_status_now();
        let head = crate::git::head_commit_id(&self.tab.root);
        let mut dirty: std::collections::HashMap<PathBuf, Option<Vec<u8>>> =
            std::collections::HashMap::new();
        let mut total = 0usize;
        for p in self.changed_paths() {
            let key = p.canonicalize().unwrap_or_else(|_| p.clone());
            // Stat pre-check: skip reading large files, use None (→ degrade to full diff) instead. Don't read then discard.
            let size = std::fs::metadata(&p).map(|m| m.len() as usize).unwrap_or(0);
            if size > FOLLOW_BASELINE_FILE_CAP
                || total.saturating_add(size) > FOLLOW_BASELINE_TOTAL_CAP
            {
                dirty.insert(key, None);
                continue;
            }
            match std::fs::read(&p) {
                Ok(content) => {
                    total = total.saturating_add(content.len());
                    dirty.insert(key, Some(content));
                }
                Err(_) => {
                    dirty.insert(key, None);
                }
            }
        }
        self.follow_baseline = Some(FollowBaseline { dirty, head });
    }

    /// Whether `follow_session`/`follow_baseline` still describe the tab's *current* root — cheap
    /// (no I/O), so it's safe to call from the render path. `false` means the root moved out from
    /// under the session since it was (re)captured (tab switch, `l`/`h`, worktree switch, paste-jump,
    /// a bookmark jump, ...); consumers must degrade rather than serve stale-repo data. Only
    /// `follow_note_change` (the event-drain side) acts on a `false` result by recapturing.
    pub(super) fn follow_scope_valid(&self) -> bool {
        self.follow_root.as_deref() == Some(self.tab.root.as_path())
    }

    /// The follow diff for `path`: baseline (follow-start) content vs the current on-disk content, as
    /// `DiffLine`s for the existing GitDiff renderer. None (→ caller uses the full git diff) when there
    /// is no baseline session, the session belongs to a different root than the current tab (see
    /// `follow_scope_valid` — critically, this also catches a same-tab root switch between linked
    /// worktrees, where `blob_at` would otherwise *successfully* resolve against the wrong worktree's
    /// HEAD and produce a plausible-looking but wrong diff), the file was dirty-but-too-large at
    /// follow-start, the current file is unreadable / too large, or either side is non-UTF-8 (binary).
    #[cfg(feature = "git")]
    pub(super) fn follow_baseline_diff(&self, path: &Path) -> Option<Vec<crate::git::DiffLine>> {
        if !self.follow_scope_valid() {
            return None;
        }
        let base = self.follow_baseline.as_ref()?;
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let baseline: Vec<u8> = match base.dirty.get(&key) {
            Some(Some(content)) => content.clone(),
            Some(None) => return None, // dirty at follow-start but too large → full diff
            None => {
                // Clean at follow-start → HEAD blob; a file created after follow-start is absent from
                // the pinned tree → empty baseline = all-added (correct: it is new since follow-start).
                // No HEAD to diff against (not a repo / unborn) → `?` returns None so `compute_gitdiff_lines`
                // defers to `file_diff`, which is empty outside a repo → `follow_jump` shows the file preview.
                let h = base.head.as_deref()?;
                crate::git::blob_at(&self.tab.root, h, path).unwrap_or_default()
            }
        };
        let current = std::fs::read(path).ok()?;
        if current.len() > FOLLOW_BASELINE_FILE_CAP {
            return None;
        }
        let old = String::from_utf8(baseline).ok()?;
        let new = String::from_utf8(current).ok()?;
        Some(crate::git::diff_contents(&old, &new))
    }

    /// `f` in a follow-opened diff: switch between the diff-since-follow-start (default) and the full
    /// git diff for the same file. No-op in a non-follow diff (already full). Drops the diff cache so
    /// the view recomputes in the new scope.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn toggle_follow_diff_scope(&mut self) {
        if self.is_git_diff_preview() && self.diff_follow_scope {
            self.follow_diff_full = !self.follow_diff_full;
            self.diff_cache = None;
            let msg = if self.follow_diff_full {
                crate::i18n::Msg::FollowShowFull
            } else {
                crate::i18n::Msg::FollowShowSince
            };
            self.flash = Some(tr(self.lang, msg).into());
        }
    }

    /// The scope label for a follow-opened diff title/flash: `FollowShowSince` (baseline) or
    /// `FollowShowFull` (toggled). None for non-follow diffs (no suffix).
    pub fn follow_diff_scope_msg(&self) -> Option<crate::i18n::Msg> {
        if self.is_git_diff_preview() && self.diff_follow_scope {
            Some(if self.follow_diff_full {
                crate::i18n::Msg::FollowShowFull
            } else {
                crate::i18n::Msg::FollowShowSince
            })
        } else {
            None
        }
    }

    /// Whether follow mode is on (chip display / the run loop's jump gate).
    pub fn follow_enabled(&self) -> bool {
        self.follow_mode
    }

    /// Stop following. Called when the user leaves the follow diff with `q` or enters a text/modal
    /// surface (`main.rs::handle_key`). Follow is otherwise sticky — scroll/`n`/`N`/`f` keep it on, and
    /// `F` toggles it off via `toggle_follow`. Re-enable with one `F`. Flash so the change is visible.
    pub fn follow_break(&mut self) {
        if self.follow_mode {
            self.follow_mode = false;
            self.flash = Some(tr(self.lang, crate::i18n::Msg::FollowOff).into());
        }
    }

    /// Follow-mode jump: reveal `path` in the tree and open its preview. Gated to meaningful targets:
    /// under root, an existing visible file, not gitignored, not already being previewed (the preview
    /// auto-reload handles content changes of the current file). No-op outside Tree/Preview surfaces
    /// (dialogs and git views are never hijacked; follow is simply paused there and resumes on return).
    pub fn follow_jump(&mut self, path: &Path) {
        use crate::keymap::Surface;
        if !self.follow_mode {
            return;
        }
        // Keep following into the next file even from the diff view (PreviewGitDiff) that follow itself opened.
        // Other git views/dialogs are never hijacked (normally a keypress breaks follow before they're reached).
        let surface_ok = matches!(
            self.surface(),
            Surface::Tree | Surface::PreviewText | Surface::PreviewImage | Surface::PreviewTable
        );
        #[cfg(feature = "git")]
        let surface_ok = surface_ok || matches!(self.surface(), Surface::PreviewGitDiff);
        if !surface_ok {
            return;
        }
        if self.tab.preview_path.as_deref() == Some(path) {
            return;
        }
        if !self.follow_target_ok(path) {
            return;
        }
        // The moment following moves the view, clear the old flash (e.g. "follow: on") so the footer is
        // handed over to that surface's operation hints (flash normally clears on a keypress, but following proceeds without one).
        self.flash = None;
        if self.tab.changed_filter {
            // Refresh the changed-file list before selecting the target (it should be in the list — it came from a change event).
            // `None`: the cursor is moved onto `path` on the very next line, so there is nothing to
            // follow — and `entries` here may already be the ordinary tree (the refresh that brought
            // us here defers the list rebuild whenever a status scan is in flight), which is exactly
            // the value that must not be mistaken for a selection in the changed list.
            self.reapply_changed_filter(None);
            if let Some(i) = self.tab.entries.iter().position(|e| e.path == path) {
                self.tab.selected = i;
            }
        } else if !matches!(self.reveal_path_deep(path), Ok(true)) {
            return;
        }
        // By default (`ui.follow_view="diff"`) this opens as a **full-screen diff** so what changed is
        // visible hunk-by-hunk (the same presentation as hunk/livediff/diffpane). Untracked files (all
        // lines new) that can't produce a diff, files outside the repository, and binary media fall
        // back to the file preview (+ scroll to the first changed hunk).
        if self.cfg.ui.follow_view != "file" && !self.follow_is_media(path) {
            // Follow-originated → baseline diff since start (or the conventional full diff if follow_diff_full).
            let diff = self.compute_gitdiff_lines(path, true);
            if !diff.is_empty() {
                self.open_git_diff(path);
                // Put the diff we just took into the cache to avoid re-fetching (re-running git) on render.
                self.diff_cache = Some(DiffCache {
                    path: path.to_path_buf(),
                    lines: diff,
                });
                // Follow-originated diff: n/N and the position indicator cycle through "files changed during this session".
                self.diff_follow_scope = true;
                return;
            }
        }
        self.enter_preview(path);
        self.follow_scroll_to_first_change();
    }

    /// Whether `path` is a valid follow target: under root, an existing file, not gitignored, and not
    /// inside a hidden (dot) directory unless hidden files are shown. Shared by the jump and the
    /// session-list recording so both see the same set.
    fn follow_target_ok(&self, path: &Path) -> bool {
        if !path.starts_with(&self.tab.root) || !path.is_file() || self.is_ignored(path) {
            return false;
        }
        if !self.tab.show_hidden {
            // Hidden (dot) directories/files can't be shown in the tree, so don't follow into them (show_hidden lifts this).
            let hidden = path
                .strip_prefix(&self.tab.root)
                .map(|r| {
                    r.components().any(|c| {
                        c.as_os_str()
                            .to_str()
                            .map(|s| s.starts_with('.'))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(true);
            if hidden {
                return false;
            }
        }
        true
    }

    /// Record one changed file into the current follow session (called from the run loop's event drain
    /// while follow is ON). Returns whether the path is a valid follow target (the caller uses this to
    /// decide whether it may become the pending jump target). First-change order, deduped.
    ///
    /// **The only place that recaptures a stale follow scope.** The root can change out from under an
    /// ON follow session without ever going through `toggle_follow` (tab switch, `l`/`h`, worktree
    /// switch via `o`→`w`→`Enter`, paste-jump, a bookmark jump, ...). When that happens the session and
    /// baseline still describe the *old* root, so before recording anything here they're rebuilt fresh
    /// against the current root — the same as a fresh `F`-on — so `n`/`N` and any follow diff never mix
    /// paths from two repos (see `follow_root`'s doc comment). This is deliberately confined to the
    /// event-drain side: `follow_baseline_diff`/`follow_session_paths` (read from the render path) only
    /// ever consult `follow_scope_valid` and degrade — they never trigger a git-status call themselves.
    pub fn follow_note_change(&mut self, path: &Path) -> bool {
        if !self.follow_mode {
            return false;
        }
        if !self.follow_scope_valid() {
            self.follow_session.clear();
            self.follow_diff_full = false;
            self.diff_cache = None;
            #[cfg(feature = "git")]
            self.capture_follow_baseline();
            self.follow_root = Some(self.tab.root.clone());
        }
        if !self.follow_target_ok(path) {
            return false;
        }
        if !self.follow_session.iter().any(|p| p == path) {
            self.follow_session.push(path.to_path_buf());
        }
        true
    }

    /// Whether `path` previews as media (image/SVG/video/PDF) — its git diff would be a useless
    /// "binary files differ" line, so follow shows the content preview instead.
    pub(super) fn follow_is_media(&self, path: &Path) -> bool {
        matches!(
            self.cfg.resolve_preview(path),
            PreviewKind::Image(_)
                | PreviewKind::Svg(_)
                | PreviewKind::Video(_)
                | PreviewKind::Pdf(_)
        )
    }

    /// After a follow jump, scroll the (windowed) preview to the **first changed hunk** instead of the
    /// file top — an edit deep in a large file would otherwise be off-screen and invisible, which defeats
    /// "watch the agent work" (Zed follows the edit position; diffpane scrolls to the latest change).
    /// A few context lines are kept above, and the caret lands on the changed line (ready for `v`/`Y`).
    /// No-ops for non-windowed previews, untracked files (all-new → top is right), and outside a repo.
    fn follow_scroll_to_first_change(&mut self) {
        if !self.is_windowed() {
            return;
        }
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        // Get the changed lines even with the gutter setting OFF (if ON, the same computation is cached in gutter_cache and reused for rendering).
        let marks = if self.cfg.ui.git_gutter {
            self.git_gutter_marks()
        } else {
            gutter_marks(&crate::git::file_diff(&self.tab.root, &path))
        };
        let Some(first) = marks.keys().min().copied() else {
            return;
        };
        let line0 = (first as usize).saturating_sub(1); // marks are 1-based
        let top = line0.saturating_sub(3); // keep a few context lines above
        let Some(win) = self.preview_win.as_mut() else {
            return;
        };
        if let Ok((off, _)) = win.advance(0, top) {
            self.tab.preview_byte_top = off;
            self.tab.preview_top_line = top;
            self.tab.preview_cursor_line = line0;
        }
    }
}
