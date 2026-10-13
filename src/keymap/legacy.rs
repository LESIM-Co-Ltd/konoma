//! The pre-restructure keymap, kept verbatim as the oracle for the equivalence tests.
//!
//! Until plugin stage 2 / B1 the key tables were one hand-written table per `Surface`. B1 rebuilt
//! them as layered per-context tables with the default keys unchanged; this module is the *old*
//! implementation (defaults, config merge, conflict check, resolve) so that
//! `equivalence_tests` can compare every (surface, state, key) lookup of the new layered keymap
//! against it. B3 changes the default keys on purpose and replaces these tests with ones that pin
//! the new defaults, at which point this file is deleted.

use super::*;

/// Old keymap: one table per surface plus `global`.
#[derive(Debug, Clone)]
pub(super) struct LegacyKeyMap {
    pub per_surface: HashMap<Surface, ContextMap>,
    pub global: ContextMap,
    pub leaders: HashMap<LeaderId, LeaderMenu>,
    pub conflicts: Vec<KeyConflict>,
    pub warnings: Vec<String>,
}

impl LegacyKeyMap {
    /// Build the built-in default keymap (§2). `scheme` only swaps the page/half rows of the Preview surfaces.
    pub fn defaults(scheme: KeyScheme) -> LegacyKeyMap {
        let nav = |m: Motion| Binding::Run(Action::Navigate(m));
        let run = |a: Action| Binding::Run(a);

        let mut per_surface: HashMap<Surface, ContextMap> = HashMap::new();

        // --- Tree (normal) ---
        let mut tree: ContextMap = HashMap::new();
        tree.insert(KeyPress::ch('j'), nav(Motion::Down));
        tree.insert(KeyPress::ch('k'), nav(Motion::Up));
        tree.insert(KeyPress::ch('g'), nav(Motion::Top));
        tree.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        tree.insert(KeyPress::ctrl_ch('d'), nav(Motion::HalfDown));
        tree.insert(KeyPress::ctrl_ch('u'), nav(Motion::HalfUp));
        tree.insert(KeyPress::ctrl_ch('f'), nav(Motion::PageDown));
        tree.insert(KeyPress::ctrl_ch('b'), nav(Motion::PageUp));
        tree.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        tree.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        tree.insert(KeyPress::ch('h'), run(Action::TreeLeave));
        tree.insert(KeyPress::ch('l'), run(Action::TreeDescend));
        tree.insert(KeyPress::ch('q'), run(Action::CloseTabOrQuit));
        tree.insert(KeyPress::ch('/'), run(Action::FilterStart));
        tree.insert(KeyPress::ch('.'), run(Action::ToggleHidden));
        tree.insert(KeyPress::ch('r'), run(Action::Refresh));
        tree.insert(KeyPress::ch('s'), run(Action::OpenSortMenu));
        tree.insert(KeyPress::ch('i'), run(Action::ToggleInfo));
        tree.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        tree.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        tree.insert(KeyPress::ch('m'), run(Action::MarkSet));
        tree.insert(KeyPress::ch('\''), run(Action::MarkJump));
        tree.insert(KeyPress::ch('d'), run(Action::OpenGitDiffCursor));
        tree.insert(KeyPress::ch('o'), run(Action::OpenGitView));
        tree.insert(KeyPress::ch('a'), run(Action::SetAnchor));
        tree.insert(KeyPress::ch('A'), run(Action::ResetAnchor));
        tree.insert(KeyPress::ch('v'), run(Action::EnterVisual));
        tree.insert(KeyPress::ch('V'), run(Action::ToggleSelect));
        // Agent Watch: C = show only changed files / n·N = jump to the next/previous changed file.
        tree.insert(KeyPress::ch('C'), run(Action::ToggleChangedFilter));
        tree.insert(KeyPress::ch('n'), run(Action::JumpNextChange));
        tree.insert(KeyPress::ch('N'), run(Action::JumpPrevChange));
        tree.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        tree.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::File));
        // Ctrl-t = open the entry under the cursor in a new tab (file = preview / directory = the
        // new tab's root). Same key/meaning ("open this in a new tab") as preview's OpenLinkNewTab
        // — paired with global t (new tab).
        tree.insert(KeyPress::ctrl_ch('t'), run(Action::OpenInNewTab));
        per_surface.insert(Surface::Tree, tree);

        // --- Tree:Visual ---
        let mut visual: ContextMap = HashMap::new();
        visual.insert(KeyPress::ch('j'), nav(Motion::Down));
        visual.insert(KeyPress::ch('k'), nav(Motion::Up));
        visual.insert(KeyPress::ch('g'), nav(Motion::Top));
        visual.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        visual.insert(KeyPress::ctrl_ch('d'), nav(Motion::HalfDown));
        visual.insert(KeyPress::ctrl_ch('u'), nav(Motion::HalfUp));
        visual.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        visual.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        visual.insert(KeyPress::ch('v'), run(Action::VisualCommit));
        visual.insert(KeyPress::ch('a'), run(Action::VisualSelectSiblings));
        visual.insert(KeyPress::ch('A'), run(Action::VisualSelectAll));
        visual.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::File));
        visual.insert(KeyPress::ch('q'), run(Action::Quit)); // preserves the old Visual q=quit (regression guard)
        per_surface.insert(Surface::Visual, visual);

        // --- Preview: text/markdown ---
        let mut ptext: ContextMap = HashMap::new();
        ptext.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        ptext.insert(KeyPress::ch('/'), run(Action::SearchStart));
        ptext.insert(KeyPress::ch('n'), run(Action::SearchNext));
        ptext.insert(KeyPress::ch('N'), run(Action::SearchPrev));
        ptext.insert(KeyPress::ch('j'), nav(Motion::Down));
        ptext.insert(KeyPress::ch('k'), nav(Motion::Up));
        ptext.insert(KeyPress::ch('g'), nav(Motion::Top));
        ptext.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        ptext.insert(KeyPress::ch('l'), nav(Motion::Right));
        ptext.insert(KeyPress::ch('h'), nav(Motion::Left));
        ptext.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        ptext.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        ptext.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        ptext.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        ptext.insert(KeyPress::ch('v'), run(Action::PreviewEnterVisual));
        ptext.insert(KeyPress::ch('V'), run(Action::PreviewEnterVisualLine));
        ptext.insert(KeyPress::ch('R'), run(Action::ToggleMarkdownRaw));
        // Ctrl-t = open the focused Markdown link in a new tab (Enter still opens in the same tab).
        // Consistent with the TUI convention (fzf/Telescope's Ctrl-t = new tab) + konoma's t = new
        // tab. Ctrl+letter is reliable in every terminal + tmux (unlike Ctrl+Enter, no kitty
        // protocol required). No-op when no link is focused.
        ptext.insert(KeyPress::ctrl_ch('t'), run(Action::OpenLinkNewTab));
        // Ctrl-n/Ctrl-p = preview the next/previous file in tree display order (file paging). J/K
        // conflict with the image surface's PDF page paging, and Ctrl-j is indistinguishable from
        // Enter (LF) on legacy terminals/tmux, so neither is used.
        ptext.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptext.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        // Y = copy an @path#L reference for the caret line (hands the location to Claude Code). If
        // there's a selection, the range is used instead (Y in the visual surface).
        ptext.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        // +/-/= : zoom/reset the focused inline mermaid diagram in place (the display area stays
        // fixed). No-op when no diagram is focused (guarded on the image_zoom_by side). `0` is kept
        // as-is for LineHome (go to line start).
        ptext.insert(KeyPress::ch('+'), run(Action::ImageZoomIn));
        ptext.insert(KeyPress::ch('-'), run(Action::ImageZoomOut));
        ptext.insert(KeyPress::ch('='), run(Action::ImageZoomReset));
        ptext.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        ptext.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        legacy_apply_scheme_paging(&mut ptext, scheme);
        ptext.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        // Bookmarks: `m` registers the previewed file, `'` opens the list (same action as the tree).
        ptext.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptext.insert(KeyPress::ch('\''), run(Action::MarkJump));
        // Heading outline (Markdown preview). `o` toggles it open/closed.
        ptext.insert(KeyPress::ch('o'), run(Action::ToggleOutline));
        // J/K = next/previous slide of a presentation (the same keys as the PDF page / sheet turn;
        // `App::page_turn` does nothing for any other text preview).
        ptext.insert(KeyPress::ch('J'), run(Action::PageNext));
        ptext.insert(KeyPress::ch('K'), run(Action::PagePrev));
        per_surface.insert(Surface::PreviewText, ptext);

        // --- Preview: text/code visual selection (v charwise / V linewise) ---
        // h/j/k/l move the 2D caret to extend the range, `y` copies it, and v/V/q/Esc exit.
        let mut pvis: ContextMap = HashMap::new();
        pvis.insert(KeyPress::ch('j'), nav(Motion::Down));
        pvis.insert(KeyPress::ch('k'), nav(Motion::Up));
        pvis.insert(KeyPress::ch('g'), nav(Motion::Top));
        pvis.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        pvis.insert(KeyPress::ch('l'), nav(Motion::Right));
        pvis.insert(KeyPress::ch('h'), nav(Motion::Left));
        pvis.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        pvis.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        pvis.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        pvis.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        legacy_apply_scheme_paging(&mut pvis, scheme);
        pvis.insert(KeyPress::ch('y'), run(Action::PreviewCopySelection));
        pvis.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        pvis.insert(KeyPress::ch('v'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('V'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('q'), run(Action::PreviewExitVisual));
        per_surface.insert(Surface::PreviewTextVisual, pvis);

        // --- Preview: image ---
        let mut pimg: ContextMap = HashMap::new();
        pimg.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        // R: while this image/PDF/SVG preview *is* the diff's own `Preview` representation
        // (`PerTab::preview_from_diff`), returns to the diff; a no-op otherwise
        // (`App::image_return_to_diff`'s own doc comment — **not** `Action::ToggleMarkdownRaw`,
        // whose non-diff fallthrough (`toggle_md_raw`) is a real, if unadvertised, bug on this
        // surface for a standalone `.mmd` image preview, `docs/FEATURE-MEDIA-DIFF.md` §6).
        pimg.insert(KeyPress::ch('R'), run(Action::ImageReturnToDiff));
        pimg.insert(KeyPress::ch('+'), run(Action::ImageZoomIn));
        pimg.insert(KeyPress::ch('-'), run(Action::ImageZoomOut));
        pimg.insert(KeyPress::ch('0'), run(Action::ImageZoomReset));
        pimg.insert(KeyPress::ch('='), run(Action::ImageZoomReset));
        pimg.insert(KeyPress::ch('h'), nav(Motion::Left));
        pimg.insert(KeyPress::ch('l'), nav(Motion::Right));
        pimg.insert(KeyPress::ch('k'), nav(Motion::Up));
        pimg.insert(KeyPress::ch('j'), nav(Motion::Down));
        // PDF page paging (shared across the image surfaces; the handler no-ops for non-PDF).
        // lowercase jk = pan within the page / uppercase JK = move to the next/previous page.
        pimg.insert(KeyPress::ch('J'), run(Action::PageNext));
        pimg.insert(KeyPress::ch('K'), run(Action::PagePrev));
        pimg.insert(KeyPress::key(KeyCode::PageDown), run(Action::PageNext));
        pimg.insert(KeyPress::key(KeyCode::PageUp), run(Action::PagePrev));
        pimg.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        pimg.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        // Ctrl-n/Ctrl-p = file paging (J/K are already assigned to PDF page paging, hence Ctrl here).
        pimg.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        pimg.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        pimg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        pimg.insert(KeyPress::ch('m'), run(Action::MarkSet));
        pimg.insert(KeyPress::ch('\''), run(Action::MarkJump));
        per_surface.insert(Surface::PreviewImage, pimg);

        // --- Preview: table (csv/tsv) ---
        // hjkl = move the cell cursor / g·G = first/last row / 0·$ = first/last column / y→ = copy
        // the cell/row/column.
        let mut ptbl: ContextMap = HashMap::new();
        ptbl.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        ptbl.insert(KeyPress::ch('j'), nav(Motion::Down));
        ptbl.insert(KeyPress::ch('k'), nav(Motion::Up));
        ptbl.insert(KeyPress::ch('h'), nav(Motion::Left));
        ptbl.insert(KeyPress::ch('l'), nav(Motion::Right));
        ptbl.insert(KeyPress::ch('g'), nav(Motion::Top));
        ptbl.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        ptbl.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        ptbl.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        ptbl.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        ptbl.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        legacy_apply_scheme_paging(&mut ptbl, scheme);
        // J/K = next/previous sheet of a spreadsheet (the same keys as the PDF page turn; the
        // handler no-ops unless the workbook has 2+ visible sheets, so CSV/archive tables ignore them).
        ptbl.insert(KeyPress::ch('J'), run(Action::PageNext));
        ptbl.insert(KeyPress::ch('K'), run(Action::PagePrev));
        ptbl.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        ptbl.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        // Ctrl-n/Ctrl-p = file paging (same keys as text/image).
        ptbl.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptbl.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        // In-table search (same `/` n N as the text preview). The cursor jumps to the matching cell.
        ptbl.insert(KeyPress::ch('/'), run(Action::SearchStart));
        ptbl.insert(KeyPress::ch('n'), run(Action::SearchNext));
        ptbl.insert(KeyPress::ch('N'), run(Action::SearchPrev));
        ptbl.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::TableCopy));
        ptbl.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptbl.insert(KeyPress::ch('\''), run(Action::MarkJump));
        per_surface.insert(Surface::PreviewTable, ptbl);

        // --- Git-related surfaces (feature gate) ---
        #[cfg(feature = "git")]
        {
            // Preview: gitdiff
            let mut pgit: ContextMap = HashMap::new();
            pgit.insert(KeyPress::ch('q'), run(Action::PreviewBack));
            pgit.insert(KeyPress::ch('x'), run(Action::GitDiffDiscard));
            pgit.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
            pgit.insert(KeyPress::ch('R'), run(Action::CycleDiffView));
            // f = toggle the range of a follow-opened diff (since follow-start ⇄ full). No-op for
            // a non-follow diff.
            pgit.insert(KeyPress::ch('f'), run(Action::ToggleFollowDiffScope));
            // J/K: turn a media diff's PDF page (both sides at once). No-op outside a multi-page
            // PDF side-by-side view (`App::media_diff_page_turn`'s own gate) — PageDown/PageUp are
            // *not* rebound here: `dispatch_navigate`'s `PreviewGitDiff` arm reroutes them to the
            // same handler only while the media diff is active, and keeps their ordinary scroll
            // meaning otherwise (`docs/FEATURE-MEDIA-DIFF.md` §6).
            pgit.insert(KeyPress::ch('J'), run(Action::PageNext));
            pgit.insert(KeyPress::ch('K'), run(Action::PagePrev));
            pgit.insert(KeyPress::ch('j'), nav(Motion::Down));
            pgit.insert(KeyPress::ch('k'), nav(Motion::Up));
            pgit.insert(KeyPress::ch('l'), nav(Motion::Right));
            pgit.insert(KeyPress::ch('h'), nav(Motion::Left));
            pgit.insert(KeyPress::ch('0'), nav(Motion::LineHome));
            pgit.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
            pgit.insert(KeyPress::ch('g'), nav(Motion::Top));
            pgit.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            pgit.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
            pgit.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
            legacy_apply_scheme_paging(&mut pgit, scheme);
            // n/N = switch to the next/previous changed file's diff (cycle through changes without
            // leaving the view; same meaning as the tree's n/N).
            pgit.insert(KeyPress::ch('n'), run(Action::JumpNextChange));
            pgit.insert(KeyPress::ch('N'), run(Action::JumpPrevChange));
            pgit.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
            per_surface.insert(Surface::PreviewGitDiff, pgit);

            // Git changes hub (o). Enter→GitOpenSelectedDiff fires as a fixed key.
            let mut gchg: ContextMap = HashMap::new();
            gchg.insert(KeyPress::ch('j'), nav(Motion::Down));
            gchg.insert(KeyPress::ch('k'), nav(Motion::Up));
            gchg.insert(KeyPress::ch('s'), run(Action::GitStage));
            gchg.insert(KeyPress::ch('u'), run(Action::GitUnstage));
            gchg.insert(KeyPress::ch('S'), run(Action::GitStageAll));
            gchg.insert(KeyPress::ch('U'), run(Action::GitUnstageAll));
            gchg.insert(KeyPress::ch('x'), run(Action::GitDiscard));
            gchg.insert(KeyPress::ch('c'), run(Action::GitCommit));
            gchg.insert(KeyPress::ch('d'), run(Action::GitWorktreeDiff));
            gchg.insert(KeyPress::ch('b'), run(Action::GitOpenBranches));
            gchg.insert(KeyPress::ch('w'), run(Action::GitOpenWorktrees));
            gchg.insert(KeyPress::ch('l'), run(Action::GitOpenLog));
            gchg.insert(KeyPress::ch('g'), run(Action::GitOpenGraph));
            gchg.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
            gchg.insert(KeyPress::ch('R'), run(Action::JjSync));
            gchg.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ copies the selected changed file's path (reuses the same path-copy menu as the tree).
            gchg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
            per_surface.insert(Surface::GitChanges, gchg);

            // Git log / graph (identical). Enter→GitOpenDetail is a fixed key.
            let mut glog: ContextMap = HashMap::new();
            glog.insert(KeyPress::ch('j'), nav(Motion::Down));
            glog.insert(KeyPress::ch('k'), nav(Motion::Up));
            glog.insert(KeyPress::ch('g'), nav(Motion::Top));
            glog.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            glog.insert(KeyPress::ch('l'), run(Action::GitOpenDetail));
            glog.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ copy commit info (short/full hash, subject, full message, author, date). Shared by
            // log/graph.
            glog.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
            // The graph has base-branch pinning (Phase 2): s = set / x = clear / b = branch
            // visibility panel. log has none of this, hence a separate map.
            let mut ggraph = glog.clone();
            ggraph.insert(KeyPress::ch('s'), run(Action::GitGraphSetBase));
            ggraph.insert(KeyPress::ch('a'), run(Action::GitGraphToggleAll));
            ggraph.insert(KeyPress::ch('x'), run(Action::GitGraphClearBase));
            // `0` also clears it (the vim feel of "line-home = return to the anchor". Coexists with
            // `x`; both resolve to the same Action).
            ggraph.insert(KeyPress::ch('0'), run(Action::GitGraphClearBase));
            ggraph.insert(KeyPress::ch('b'), run(Action::GitGraphOpenPicker));
            per_surface.insert(Surface::GitGraph, ggraph);
            per_surface.insert(Surface::GitLog, glog);

            // The graph's branch visibility panel: j/k/g/G navigate + Space:toggle / a:all /
            // n:current-only. Enter:apply / q·Esc:cancel are fixed keys.
            let mut gpick: ContextMap = HashMap::new();
            gpick.insert(KeyPress::ch('j'), nav(Motion::Down));
            gpick.insert(KeyPress::ch('k'), nav(Motion::Up));
            gpick.insert(KeyPress::ch('g'), nav(Motion::Top));
            gpick.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gpick.insert(KeyPress::ch(' '), run(Action::GitGraphPickerToggle));
            gpick.insert(KeyPress::ch('a'), run(Action::GitGraphPickerAll));
            gpick.insert(KeyPress::ch('n'), run(Action::GitGraphPickerCurrentOnly));
            // Reorder priority (uppercase J/K). Lowercase j/k move the cursor.
            gpick.insert(KeyPress::ch('K'), run(Action::GitGraphPickerMoveUp));
            gpick.insert(KeyPress::ch('J'), run(Action::GitGraphPickerMoveDown));
            gpick.insert(KeyPress::ch('q'), run(Action::GitClose));
            per_surface.insert(Surface::GitGraphPicker, gpick);

            // Git branches. Enter→BranchCheckout is a fixed key.
            let mut gbr: ContextMap = HashMap::new();
            gbr.insert(KeyPress::ch('j'), nav(Motion::Down));
            gbr.insert(KeyPress::ch('k'), nav(Motion::Up));
            gbr.insert(KeyPress::ch('g'), nav(Motion::Top));
            gbr.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gbr.insert(KeyPress::ch('/'), run(Action::BranchFilterStart));
            gbr.insert(KeyPress::ch('l'), run(Action::BranchCheckout));
            gbr.insert(KeyPress::ch('n'), run(Action::BranchCreate));
            gbr.insert(KeyPress::ch('d'), run(Action::BranchDelete));
            gbr.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ copy the selected branch name (there's only one target, so it copies directly with
            // no menu).
            gbr.insert(KeyPress::ch('y'), run(Action::CopyBranchName));
            per_surface.insert(Surface::GitBranches, gbr);

            // Git worktrees (`w` from the changes hub). `Enter`→worktree_goto and `Esc`→
            // worktree_close (via handle_esc) are fixed keys (same idiom as branches' Enter→
            // checkout); `WorktreeGoto` has no default key binding here, same as
            // `GitOpenSelectedDiff` — it still round-trips through action_from_str/action_name so a
            // user can rebind another key to it. Ctrl-t (config-rebindable, unlike Enter) opens the
            // selection in a new tab. `n` opens the "new worktree" input dialog, same idiom as
            // `BranchCreate`'s `n` in `[keys.git_branches]` below.
            let mut gwt: ContextMap = HashMap::new();
            gwt.insert(KeyPress::ch('j'), nav(Motion::Down));
            gwt.insert(KeyPress::ch('k'), nav(Motion::Up));
            gwt.insert(KeyPress::ch('g'), nav(Motion::Top));
            gwt.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gwt.insert(KeyPress::ch('/'), run(Action::WorktreeFilterStart));
            gwt.insert(KeyPress::ch('n'), run(Action::WorktreeCreate));
            gwt.insert(KeyPress::ctrl_ch('t'), run(Action::WorktreeGotoNewTab));
            gwt.insert(KeyPress::ch('d'), run(Action::WorktreeShowChanges));
            gwt.insert(KeyPress::ch('q'), run(Action::WorktreeClose));
            per_surface.insert(Surface::GitWorktrees, gwt);

            // Git detail (all Motions enabled; page/half are swapped per scheme).
            let mut gdet: ContextMap = HashMap::new();
            gdet.insert(KeyPress::ch('j'), nav(Motion::Down));
            gdet.insert(KeyPress::ch('k'), nav(Motion::Up));
            gdet.insert(KeyPress::ch('l'), nav(Motion::Right));
            gdet.insert(KeyPress::ch('h'), nav(Motion::Left));
            gdet.insert(KeyPress::ch('0'), nav(Motion::LineHome));
            gdet.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
            gdet.insert(KeyPress::ch('g'), nav(Motion::Top));
            gdet.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gdet.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
            gdet.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
            gdet.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
            gdet.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ copy commit info (can copy while reading the full message).
            gdet.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
            legacy_apply_scheme_paging(&mut gdet, scheme);
            per_surface.insert(Surface::GitDetail, gdet);
        }

        // --- Sort menu (scheme-independent) ---
        let mut sort: ContextMap = HashMap::new();
        sort.insert(KeyPress::ch('n'), run(Action::SortSet(SortKey::Name)));
        sort.insert(KeyPress::ch('s'), run(Action::SortSet(SortKey::Size)));
        sort.insert(KeyPress::ch('m'), run(Action::SortSet(SortKey::Modified)));
        sort.insert(KeyPress::ch('e'), run(Action::SortSet(SortKey::Ext)));
        sort.insert(KeyPress::ch('r'), run(Action::SortToggleReverse));
        sort.insert(KeyPress::ch('.'), run(Action::SortToggleDirsFirst));
        per_surface.insert(Surface::Sort, sort);

        // --- Bookmark list. Enter→BookmarkJump is a fixed key. Plain letters are reserved for
        // bookmark-name jumps (main's Unbound fallback), so edit/delete are placed on Ctrl-modified
        // keys instead. ---
        let mut bm: ContextMap = HashMap::new();
        bm.insert(KeyPress::ch('j'), nav(Motion::Down));
        bm.insert(KeyPress::ch('k'), nav(Motion::Up));
        bm.insert(KeyPress::ctrl_ch('e'), run(Action::BookmarkEdit));
        bm.insert(KeyPress::ctrl_ch('d'), run(Action::BookmarkDelete));
        bm.insert(KeyPress::ch('q'), run(Action::BookmarkClose));
        bm.insert(KeyPress::ch('\''), run(Action::BookmarkClose));
        per_surface.insert(Surface::Bookmarks, bm);

        // --- Tab list (`T` toggles it open/closed. Enter=switch is a fixed key; T closes it via
        // Global inheritance) ---
        let mut tl: ContextMap = HashMap::new();
        tl.insert(KeyPress::ch('j'), nav(Motion::Down));
        tl.insert(KeyPress::ch('k'), nav(Motion::Up));
        tl.insert(KeyPress::ch('d'), run(Action::TabListClose));
        tl.insert(KeyPress::ch('q'), run(Action::ToggleTabList));
        per_surface.insert(Surface::Tabs, tl);

        // --- Heading outline list (`o` toggles it open/closed. Enter=jump is a fixed key) ---
        let mut outline: ContextMap = HashMap::new();
        outline.insert(KeyPress::ch('j'), nav(Motion::Down));
        outline.insert(KeyPress::ch('k'), nav(Motion::Up));
        outline.insert(KeyPress::ch('g'), nav(Motion::Top));
        outline.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        outline.insert(KeyPress::ch('q'), run(Action::ToggleOutline));
        outline.insert(KeyPress::ch('o'), run(Action::ToggleOutline));
        per_surface.insert(Surface::Outline, outline);

        // --- Info ---
        let mut info: ContextMap = HashMap::new();
        info.insert(KeyPress::ch('i'), run(Action::InfoClose));
        info.insert(KeyPress::ch('q'), run(Action::InfoClose));
        per_surface.insert(Surface::Info, info);

        // --- Table cell full-text popup (`Enter` in a table preview opens it; fixed key, see
        // handle_enter). j/k/g/G/PageUp/PageDown scroll the (possibly wrapped/long) cell text.
        // `q` closes (Enter/Esc also close via the fixed-key path). ---
        let mut tcell: ContextMap = HashMap::new();
        tcell.insert(KeyPress::ch('j'), nav(Motion::Down));
        tcell.insert(KeyPress::ch('k'), nav(Motion::Up));
        tcell.insert(KeyPress::ch('g'), nav(Motion::Top));
        tcell.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        tcell.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        tcell.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        tcell.insert(KeyPress::ch('q'), run(Action::ToggleTableCell));
        per_surface.insert(Surface::TableCell, tcell);

        // --- Help (?/Esc close it via Global/fixed keys. `q` closes it here too) ---
        let mut help: ContextMap = HashMap::new();
        help.insert(KeyPress::ch('j'), nav(Motion::Down));
        help.insert(KeyPress::ch('k'), nav(Motion::Up));
        help.insert(KeyPress::ch('g'), nav(Motion::Top));
        help.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        help.insert(KeyPress::ch('q'), run(Action::ToggleHelp));
        per_surface.insert(Surface::Help, help);

        // --- Global (inherited by allows_tabs surfaces) ---
        let mut global: ContextMap = HashMap::new();
        // Q = quit the whole app. Every allows_tabs() surface (all but text-input/confirm-modal)
        // inherits global, so Q exits from any surface. Changeable via `[keys.global]`; conflicts
        // are detected by validate() at startup.
        global.insert(KeyPress::ch('Q'), run(Action::Quit));
        // F = follow mode (auto-jump to externally changed files). Global so it can be toggled from
        // either Tree or Preview.
        global.insert(KeyPress::ch('F'), run(Action::ToggleFollow));
        global.insert(KeyPress::ch('t'), run(Action::TabNew));
        // I = "insert" a new tab next to this one (tab_new_after), landing right of the active
        // tab instead of at the end. Chosen because it's a letter no tab-allowing surface uses
        // for anything else, so it's never shadowed by a surface-specific binding; `t` keeps
        // appending at the end (unchanged for every existing caller).
        global.insert(KeyPress::ch('I'), run(Action::TabNewAfter));
        global.insert(KeyPress::ch('T'), run(Action::ToggleTabList));
        // P = read a path/GitHub link from the clipboard and jump there (reveal + preview). Global
        // so it works from either Tree or Preview. Changeable via `[keys.global]`.
        global.insert(KeyPress::ch('P'), run(Action::PasteJump));
        // No default binding is placed on `w`: vim's word-motion habit makes it easy to trigger by
        // accident, so closing a tab is consolidated onto the tree's `q` (CloseTabOrQuit) (user
        // decision, 2026-07-07). The `tab_close` action itself still exists, so it can be revived
        // via `[keys.global] w = "tab_close"`.
        global.insert(KeyPress::ch('['), run(Action::TabPrev));
        global.insert(KeyPress::ch(']'), run(Action::TabNext));
        for i in 1..=9u8 {
            let c = char::from(b'0' + i);
            global.insert(KeyPress::ch(c), run(Action::TabGoto(i - 1)));
        }
        global.insert(KeyPress::ch('?'), run(Action::ToggleHelp));

        // --- Leaders ---
        let mut leaders: HashMap<LeaderId, LeaderMenu> = HashMap::new();
        leaders.insert(LeaderId::Copy, copy_leader_default());
        leaders.insert(LeaderId::File, file_leader_default());
        leaders.insert(LeaderId::TableCopy, table_copy_leader_default());
        #[cfg(feature = "git")]
        leaders.insert(LeaderId::GitCopy, git_copy_leader_default());

        LegacyKeyMap {
            per_surface,
            global,
            leaders,
            conflicts: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// defaults → merge config → validate conflicts (§6/§7). Always falls back and returns green (never panics).
    pub fn from_config(scheme: KeyScheme, cfg: &KeysFileConfig) -> LegacyKeyMap {
        let mut map = LegacyKeyMap::defaults(scheme);
        let defaults = LegacyKeyMap::defaults(scheme); // kept aside for rollback.
        let mut warnings: Vec<String> = Vec::new();

        for (sfc_name, table) in &cfg.surfaces {
            let target = match legacy_key_target_from_name(sfc_name) {
                Some(t) => t,
                None => {
                    warnings.push(format!("unknown key surface: {sfc_name}"));
                    continue;
                }
            };
            for (chord_str, action_str) in table {
                map.apply_one(target, chord_str, action_str, &mut warnings);
            }
        }

        map.validate(&defaults);
        map.warnings = warnings;
        map
    }

    /// Apply one (chord string → action string) (§6 step ③). On failure, warn and ignore.
    fn apply_one(
        &mut self,
        target: LegacyTarget,
        chord_str: &str,
        action_str: &str,
        warnings: &mut Vec<String>,
    ) {
        let chord = match KeyChord::parse(chord_str) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("invalid key {chord_str:?}: {e}"));
                return;
            }
        };
        let is_noop = action_str == "noop" || action_str == "disabled";
        // Explicitly warn if the config tries to bind the digit-fixed TabGoto (§1.2).
        if action_str == "tab_goto" {
            warnings.push("tab_goto is fixed to digit keys and cannot be rebound".into());
            return;
        }
        let action = if is_noop {
            None
        } else {
            match action_from_str(action_str) {
                Some(a) => Some(a),
                None => {
                    warnings.push(format!("unknown action: {action_str}"));
                    return;
                }
            }
        };

        match chord {
            KeyChord::Single(kp) => {
                if is_fixed_key(kp) {
                    warnings.push(format!("cannot rebind fixed key: {chord_str:?}"));
                    return;
                }
                let cmap = self.context_mut(target);
                match action {
                    Some(a) => {
                        cmap.insert(kp, Binding::Run(a));
                    }
                    None => {
                        cmap.remove(&kp);
                    }
                }
            }
            KeyChord::Chord(pre, suf) => {
                let lead = match leader_for_prefix(pre) {
                    Some(l) => l,
                    None => {
                        warnings.push(format!(
                            "unsupported chord prefix (only `space`/`y` leaders are supported): {chord_str:?}"
                        ));
                        return;
                    }
                };
                let menu = match self.leaders.get_mut(&lead) {
                    Some(m) => m,
                    None => return,
                };
                match action {
                    Some(a) => menu.set(suf, a),
                    None => menu.remove(suf),
                }
            }
        }
    }

    fn context_mut(&mut self, target: LegacyTarget) -> &mut ContextMap {
        match target {
            LegacyTarget::Global => &mut self.global,
            LegacyTarget::Surface(s) => self.per_surface.entry(s).or_default(),
        }
    }

    /// Detect post-merge conflicts and revert conflicting overrides to the defaults (§7). `defaults` is the kept default copy.
    fn validate(&mut self, defaults: &LegacyKeyMap) {
        let mut conflicts: Vec<KeyConflict> = Vec::new();
        let global_keys: Vec<KeyPress> = self.global.keys().copied().collect();
        let surfaces: Vec<Surface> = self.per_surface.keys().copied().collect();

        for sfc in surfaces {
            // --- PrefixVsSingle: a key that defaulted to a leader prefix was stolen by a Run ---
            let leader_keys: Vec<(KeyPress, LeaderId)> = defaults
                .per_surface
                .get(&sfc)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, b)| match b {
                            Binding::Leader(id) => Some((*k, *id)),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (k, id) in leader_keys {
                let cur = self.per_surface.get(&sfc).and_then(|m| m.get(&k)).cloned();
                if let Some(Binding::Run(a)) = cur {
                    conflicts.push(KeyConflict {
                        surface: sfc,
                        key: k,
                        kept: format!("leader:{}", leader_name(id)),
                        dropped: action_name(a),
                        reason: ConflictKind::PrefixVsSingle,
                    });
                    self.per_surface
                        .get_mut(&sfc)
                        .expect("surface present")
                        .insert(k, Binding::Leader(id));
                }
            }

            // --- GlobalShadow: an allows_tabs surface stole a Global default key for another Action ---
            // A per-surface specialization that the default map itself has (e.g. the tab list's
            // `w` = close the selected tab) is legitimate. Only correct it when the user config
            // steals a Global key **in a way that differs from the default**, restoring the
            // default's per-surface binding if one exists, or Global otherwise.
            if sfc.allows_tabs() {
                let shadows: Vec<(KeyPress, String, String, Option<Binding>)> = {
                    let cmap = match self.per_surface.get(&sfc) {
                        Some(m) => m,
                        None => continue,
                    };
                    let dmap = defaults.per_surface.get(&sfc);
                    global_keys
                        .iter()
                        .filter_map(|gk| {
                            let local = cmap.get(gk)?;
                            let global = self.global.get(gk);
                            if Some(local) == global {
                                return None;
                            }
                            let default_local = dmap.and_then(|m| m.get(gk));
                            if Some(local) == default_local {
                                return None; // per-surface specialization matching the default
                            }
                            let restore = default_local.cloned();
                            let kept = match &restore {
                                Some(b) => binding_name(b),
                                None => global.map(binding_name).unwrap_or_default(),
                            };
                            Some((*gk, kept, binding_name(local), restore))
                        })
                        .collect()
                };
                for (gk, kept, dropped, restore) in shadows {
                    conflicts.push(KeyConflict {
                        surface: sfc,
                        key: gk,
                        kept,
                        dropped,
                        reason: ConflictKind::GlobalShadow,
                    });
                    let m = self.per_surface.get_mut(&sfc).expect("surface present");
                    match restore {
                        Some(b) => {
                            m.insert(gk, b);
                        }
                        None => {
                            m.remove(&gk);
                        }
                    }
                }
            }
        }

        self.conflicts = conflicts;
    }

    /// surface + (leader pending) + keystroke → resolution (§1.5). One input = 1-2 HashMap lookups.
    pub fn resolve(&self, sfc: Surface, pending: Option<LeaderId>, kp: KeyPress) -> Resolution {
        if let Some(id) = pending {
            return match self.leaders.get(&id).and_then(|m| m.find(kp)) {
                Some(a) => Resolution::Action(a),
                None => Resolution::Unbound,
            };
        }
        if let Some(b) = self.per_surface.get(&sfc).and_then(|m| m.get(&kp)) {
            return binding_to_resolution(b);
        }
        if sfc.allows_tabs() {
            if let Some(b) = self.global.get(&kp) {
                return binding_to_resolution(b);
            }
        }
        Resolution::Unbound
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyTarget {
    Global,
    Surface(Surface),
}

/// Old surface name (snake_case) → target.
fn legacy_key_target_from_name(name: &str) -> Option<LegacyTarget> {
    let s = match name {
        "global" => return Some(LegacyTarget::Global),
        "tree" => Surface::Tree,
        "tree_visual" => Surface::Visual,
        "preview_text" => Surface::PreviewText,
        "preview_text_visual" => Surface::PreviewTextVisual,
        "preview_image" => Surface::PreviewImage,
        "preview_table" => Surface::PreviewTable,
        "sort" => Surface::Sort,
        "bookmarks" => Surface::Bookmarks,
        "tabs" => Surface::Tabs,
        "outline" => Surface::Outline,
        "info" => Surface::Info,
        "table_cell" => Surface::TableCell,
        "help" => Surface::Help,
        #[cfg(feature = "git")]
        "preview_git_diff" => Surface::PreviewGitDiff,
        #[cfg(feature = "git")]
        "git_changes" => Surface::GitChanges,
        #[cfg(feature = "git")]
        "git_log" => Surface::GitLog,
        #[cfg(feature = "git")]
        "git_graph" => Surface::GitGraph,
        #[cfg(feature = "git")]
        "git_graph_picker" => Surface::GitGraphPicker,
        #[cfg(feature = "git")]
        "git_branches" => Surface::GitBranches,
        #[cfg(feature = "git")]
        "git_worktrees" => Surface::GitWorktrees,
        #[cfg(feature = "git")]
        "git_detail" => Surface::GitDetail,
        _ => return None,
    };
    Some(LegacyTarget::Surface(s))
}

fn legacy_apply_scheme_paging(m: &mut ContextMap, scheme: KeyScheme) {
    match scheme {
        KeyScheme::Vim => {
            m.insert(
                KeyPress::ctrl_ch('f'),
                Binding::Run(Action::Navigate(Motion::PageDown)),
            );
            m.insert(
                KeyPress::ctrl_ch('b'),
                Binding::Run(Action::Navigate(Motion::PageUp)),
            );
            m.insert(
                KeyPress::ctrl_ch('d'),
                Binding::Run(Action::Navigate(Motion::HalfDown)),
            );
            m.insert(
                KeyPress::ctrl_ch('u'),
                Binding::Run(Action::Navigate(Motion::HalfUp)),
            );
        }
        KeyScheme::Less => {
            m.insert(
                KeyPress::ch('f'),
                Binding::Run(Action::Navigate(Motion::PageDown)),
            );
            m.insert(
                KeyPress::ch(' '),
                Binding::Run(Action::Navigate(Motion::PageDown)),
            );
            m.insert(
                KeyPress::ch('b'),
                Binding::Run(Action::Navigate(Motion::PageUp)),
            );
            m.insert(
                KeyPress::ch('d'),
                Binding::Run(Action::Navigate(Motion::HalfDown)),
            );
            m.insert(
                KeyPress::ch('u'),
                Binding::Run(Action::Navigate(Motion::HalfUp)),
            );
        }
    }
}
