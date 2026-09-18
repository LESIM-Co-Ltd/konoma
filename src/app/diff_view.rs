//! The full-screen diff's three presentations (`docs/FEATURE-MD-RENDERED-DIFF.md`): which one a
//! given diff target can show (`representation_count`/`round_diff_view`), `R`'s cycle
//! (`cycle_diff_view`), and the footer/help hints that only ever advertise a key that would
//! actually do something (`docs/FEATURE-MD-RENDERED-DIFF.md` §4, [[hint-shown-iff-key-acts]]).
//!
//! The `Rendered` presentation draws through the **same** decorated-Markdown machinery an
//! ordinary preview does — `App::ensure_md_cache`'s own `DecoratedSource::Diff` branch
//! (`md_render.rs`) — rather than a cache/render path of its own, so real images/mermaid
//! diagrams/math render there exactly as they do everywhere else. This file owns the *state
//! machine* around all three presentations, not the rendering of any one of them.

use super::*;

impl App {
    /// Test-only view of the current diff presentation — `Source`/`Rendered`/`Preview`
    /// (`docs/FEATURE-MD-RENDERED-DIFF.md` §1/§4). Meaningless (but harmless to read) outside a
    /// GitDiff preview or its own `Preview` representation. Every e2e test that reads this is
    /// itself `#[cfg(feature = "git")]` (the presentation state only ever moves on a `git`-feature
    /// build), so this is unused — not unreachable — on a no-`git` test build.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn diff_view_for_test(&self) -> DiffView {
        self.tab.diff_view
    }

    /// Test-only view of `PerTab::preview_from_diff`. See `diff_view_for_test`'s own doc comment
    /// for why this is `allow(dead_code)`, not `cfg(feature = "git")`, on a no-`git` test build.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn preview_from_diff_for_test(&self) -> bool {
        self.tab.preview_from_diff
    }

    /// Test-only view of the `Rendered` presentation's own gutter marks (row range, `DiffMark`) —
    /// `MdCache::diff_marks`, `None` when there is no such cache built yet (or the cache on hand is
    /// `MdCacheSource::File`, which never populates this field). See `diff_view_for_test`'s own doc
    /// comment for why this is `allow(dead_code)`, not `cfg(feature = "git")`, on a no-`git` test
    /// build.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn diff_rendered_marks_for_test(
        &self,
    ) -> Option<Vec<(std::ops::Range<usize>, crate::preview::markdown::DiffMark)>> {
        self.md_cache.as_ref().map(|c| c.diff_marks.clone())
    }

    /// Test-only view of the ordinary decorated Markdown preview's own gutter marks —
    /// `MdCache::preview_gutter_marks`, `None` when there is no such cache built yet. See
    /// `diff_view_for_test`'s own doc comment for why this is `allow(dead_code)`, not `cfg(feature
    /// = "git")`, on a no-`git` test build.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn md_diff_marks_for_test(
        &self,
    ) -> Option<
        Vec<(
            std::ops::Range<usize>,
            crate::preview::markdown::PreviewMark,
        )>,
    > {
        self.md_cache
            .as_ref()
            .map(|c| c.preview_gutter_marks.clone())
    }

    /// Test-only view of the decorated Markdown cache's own key `width` (the *viewport* width it
    /// was built for — `MdCache::width`, not the possibly-1-narrower internal render width the
    /// gutter uses; see `App::ensure_md_cache`'s own `render_width`). Lets a test that only knows
    /// the terminal's own total column count (`Sim::with_config_sized`) reproduce the exact same
    /// cache the real render already built, instead of guessing the border/gutter arithmetic
    /// itself and risking silently rebuilding a *different* cache at the wrong width. See
    /// `diff_view_for_test`'s own doc comment for why this is `allow(dead_code)`, not `cfg(feature
    /// = "git")`, on a no-`git` test build.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn md_cache_width_for_test(&self) -> Option<u16> {
        self.md_cache.as_ref().map(|c| c.width)
    }

    /// Drops every cache whose content depends on "what did the working tree used to look like" —
    /// the raw diff (`DiffCache`) and, when it is currently the diff's `Rendered` presentation
    /// (`MdCacheSource::Diff` — the `Rendered` presentation now builds into `MdCache` just like an
    /// ordinary preview does, see this module's own doc comment), the decoration cache too. One
    /// name for the five call sites that used to set only `self.diff_cache = None` on their own (a
    /// new file's diff opened, the working tree changed, a follow session (re)started) — see
    /// `git_view.rs::open_git_diff`, `follow.rs` (×3), and `bookmark_actions.rs::jump_changed`'s
    /// refresh path — each of which also needs a decorated Markdown `Rendered` view rebuilt against
    /// the *new* baseline (`f` toggling since-follow-start ⇄ full git diff, say) and would otherwise
    /// have had to remember a second line to do it.
    ///
    /// Deliberately does **not** clear an ordinary (`MdCacheSource::File`) `md_cache` — that cache
    /// has its own, narrower invalidation (`App::reload_preview`, gated on `preview_affected_by`)
    /// precisely so an unrelated FS event during heavy agent file churn does not force a re-render
    /// of whatever happens to be on screen (`refresh_fs_inner`'s own "hot path" doc comment); a
    /// blanket clear here would have silently defeated that (confirmed: broke
    /// `app::tests::preview_reloads_only_for_relevant_fs_changes` when first tried).
    pub(super) fn invalidate_diff_caches(&mut self) {
        self.diff_cache = None;
        if matches!(&self.md_cache, Some(c) if c.source == MdCacheSource::Diff) {
            self.md_cache = None;
        }
    }

    /// How many of the three presentations (`docs/FEATURE-MD-RENDERED-DIFF.md` §1) `path`'s diff
    /// can actually show: `3` for Markdown (source/rendered/preview), `2` for a windowed-capable
    /// text kind (source/preview — code, plain text, a `.mmd` file, or a text-mode delegated
    /// command), `1` for anything else (an image/PDF/video/archive/table/unsupported file, or an
    /// image-mode delegated command — none of these has a second way to look at its diff at all).
    /// Judged from `self.cfg.resolve_preview(path)` — the same config-driven rule every other
    /// preview-kind decision in this codebase goes through (never a hardcoded extension list).
    pub(super) fn diff_representation_count(&self, path: &Path) -> u8 {
        match self.cfg.resolve_preview(path) {
            PreviewKind::Markdown(_) => 3,
            PreviewKind::Code(_) | PreviewKind::Text(_) | PreviewKind::Mermaid(_) => 2,
            PreviewKind::Command { render_as, .. } if render_as.as_deref() != Some("image") => 2,
            _ => 1,
        }
    }

    /// `view` rounded down to what `path` can actually show (`diff_representation_count`):
    /// `Rendered` on a file with no block-diff renderer (anything but Markdown) rounds to `Source`
    /// — "rendered が無いので source として振る舞う" (§1's own table); `Preview` on a file with no
    /// representable content preview at all (count `1`) rounds to `Source` too, since there is
    /// nothing else to show it in; `Source` itself never needs rounding.
    pub(super) fn round_diff_view(&self, view: DiffView, path: &Path) -> DiffView {
        let n = self.diff_representation_count(path);
        match view {
            DiffView::Rendered if n < 3 => DiffView::Source,
            DiffView::Preview if n < 2 => DiffView::Source,
            other => other,
        }
    }

    /// `[ui] diff_view`, resolved and rounded for `path` — what `App::open_git_diff` initializes a
    /// **freshly opened** diff to. `n`/`N` (`diff_jump_changed`) deliberately do *not* call this:
    /// they save/restore the tab's current `diff_view` around their own `open_git_diff` call so
    /// cycling files never resets the presentation (`docs/FEATURE-MD-RENDERED-DIFF.md` §4's "`n`/
    /// `N` で次のファイルの diff へ移っても表現は維持").
    pub(super) fn default_diff_view_for(&self, path: &Path) -> DiffView {
        self.round_diff_view(DiffView::parse(&self.cfg.ui.diff_view), path)
    }

    /// `R` in `Surface::PreviewGitDiff`: cycles `Source → Rendered → Preview → Source` for a
    /// Markdown target, `Source ⇄ Preview` for any other representable text kind, and does nothing
    /// at all for a target with only one representation (`diff_representation_count == 1`) — the
    /// footer/help hint (`diff_view_cycle_hint`) is hidden in exactly that last case, so a key that
    /// would do nothing is never advertised ([[hint-shown-iff-key-acts]]).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn cycle_diff_view(&mut self) {
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.clone() else {
            return;
        };
        let n = self.diff_representation_count(&path);
        if n < 2 {
            return;
        }
        let next = match self.tab.diff_view {
            DiffView::Source if n >= 3 => DiffView::Rendered,
            DiffView::Source => DiffView::Preview,
            DiffView::Rendered => DiffView::Preview,
            DiffView::Preview => DiffView::Source,
        };
        if next == DiffView::Preview {
            #[cfg(feature = "git")]
            self.enter_diff_preview_representation(&path);
            return;
        }
        // `apply_diff_view`, not a direct assignment: this is the moment `next` (`Rendered` or
        // `Source`) is decided, so validating/rounding it (and flashing why, if it does) has to
        // happen here — see that fn's own doc comment (`docs/FEATURE-MD-RENDERED-DIFF.md` §5).
        self.apply_diff_view(next, &path);
        self.tab.preview_scroll = 0;
        self.tab.diff_scroll_pending = self.tab.diff_view == DiffView::Rendered;
    }

    /// Takes (and clears) the pending "scroll to first change" request set by `App::open_git_diff`/
    /// `App::cycle_diff_view` (the `Rendered` representation, `tab.diff_view == Rendered`) or
    /// `follow.rs`'s own decorated-Markdown branch of `follow_scroll_to_first_change` (an ordinary
    /// preview's own gutter, §3) — consumed by the very next render of whichever one actually
    /// applies (`ui/preview.rs::render_diff_rendered`/`render_decorated`).
    pub(crate) fn take_diff_scroll_pending(&mut self) -> bool {
        std::mem::take(&mut self.tab.diff_scroll_pending)
    }

    /// Scrolls the decorated preview so display row `row` lands a few lines below the top — the
    /// same "keep a little context above" convention `follow_scroll_to_first_change`'s own windowed-
    /// preview branch already uses for the byte-offset case.
    pub(crate) fn scroll_preview_to_row_with_context(&mut self, row: usize) {
        let top = row.saturating_sub(3);
        self.tab.preview_scroll = top.min(u16::MAX as usize) as u16;
    }

    /// Whether the current preview *is* the diff's own `Preview` representation
    /// (`PerTab::preview_from_diff`) — `main.rs`'s `Action::PreviewBack` reads this to route `q`
    /// back into the diff (`close_git_diff`) instead of the ordinary tree return.
    pub fn preview_is_diff_representation(&self) -> bool {
        self.tab.preview_from_diff
    }

    /// Whether the diff's `Rendered` presentation is the one currently on screen —
    /// `ui/preview.rs::render_gitdiff` reads this to route into `render_diff_rendered`, and
    /// `ui/status.rs::mode_footer` reads it to drop the `s:unified/split/auto` hint a presentation
    /// with no split view of its own has no business advertising. Not itself `#[cfg(feature =
    /// "git")]` (unlike most of this presentation's machinery) precisely because `mode_footer`,
    /// compiled on every build, needs to ask it — always `false` on a no-`git` build, since nothing
    /// there can ever set `tab.diff_view` to `Rendered` in the first place (`Action::CycleDiffView`
    /// itself is feature-gated).
    pub(crate) fn diff_rendered_active(&self) -> bool {
        self.is_git_diff_preview() && self.tab.diff_view == DiffView::Rendered
    }

    /// `R`'s meaning in `Surface::PreviewGitDiff` **right now**, for the footer/help hint —
    /// `docs/FEATURE-MD-RENDERED-DIFF.md` §4's "**Markdown の path の時だけ出す**
    /// ([[hint-shown-iff-key-acts]])", generalized to "only when it would actually change anything":
    /// `None` when the target has just one representation (mirrors `cycle_diff_view`'s own no-op
    /// gate exactly), otherwise the `Msg` naming whichever presentation `R` would switch *to*.
    pub fn diff_view_cycle_hint(&self) -> Option<crate::i18n::Msg> {
        let PreviewKind::GitDiff(path) = self.tab.preview_kind.as_ref()? else {
            return None;
        };
        let n = self.diff_representation_count(path);
        if n < 2 {
            return None;
        }
        Some(match self.tab.diff_view {
            DiffView::Source if n >= 3 => crate::i18n::Msg::DiffViewRendered,
            DiffView::Source => crate::i18n::Msg::DiffViewPreview,
            DiffView::Rendered => crate::i18n::Msg::DiffViewPreview,
            DiffView::Preview => crate::i18n::Msg::DiffViewSource,
        })
    }

    /// `R` in `Surface::PreviewText`/`PreviewImage` while `preview_from_diff` is set: instead of the
    /// ordinary raw-source toggle, returns to the diff's own `Source` presentation
    /// (`docs/FEATURE-MD-RENDERED-DIFF.md` §4's "`R`＝Source の diff に戻る") — the counterpart to
    /// `main.rs`'s `Action::PreviewBack` already routing `q` back through `close_git_diff` whenever
    /// this flag is set. Falls through to the ordinary `toggle_md_raw` for every other preview
    /// (including a plain non-diff Markdown one), so rebinding nothing and adding no new keymap
    /// entry keeps this a drop-in replacement for `main.rs`'s old `Action::ToggleMarkdownRaw` arm.
    pub fn toggle_md_raw_or_return_to_diff(&mut self) {
        #[cfg(feature = "git")]
        if self.tab.preview_from_diff {
            self.return_to_diff_from_preview();
            return;
        }
        self.toggle_md_raw();
    }

    /// `R`'s meaning in `Surface::PreviewText` right now — `Some(HintDiff)` while `preview_from_diff`
    /// is set (this preview *is* the diff's `Preview` representation), otherwise the ordinary raw/
    /// rendered toggle hint `ui/preview.rs::footer_hints` already computes for every other Markdown/
    /// Mermaid preview. Kept here (rather than inlined at the one call site) so the "which case am I
    /// in" logic lives next to `toggle_md_raw_or_return_to_diff`, the key handler it mirrors.
    pub fn diff_preview_raw_hint(&self) -> Option<crate::i18n::Msg> {
        if self.tab.preview_from_diff {
            return Some(crate::i18n::Msg::HintReturnToDiff);
        }
        None
    }

    /// Enters the diff's `Preview` representation for `path` — an ordinary content preview
    /// (`App::enter_preview`), but tagged `preview_from_diff` and scrolled to the first change the
    /// same way a follow jump lands on one (`follow_scroll_to_first_change`, which this reuses
    /// directly: the `preview` presentation's own gutter, wired in `md_render.rs`/`app.rs`'s
    /// `windowed_lines`, is exactly the same change-gutter machinery a follow-opened file preview
    /// already scrolls to the first mark of).
    #[cfg(feature = "git")]
    fn enter_diff_preview_representation(&mut self, path: &Path) {
        let came_from_git_view = self.tab.came_from_git_view;
        let diff_follow_scope = self.diff_follow_scope;
        self.enter_preview(path);
        self.tab.came_from_git_view = came_from_git_view;
        self.diff_follow_scope = diff_follow_scope;
        self.tab.preview_from_diff = true;
        self.follow_scroll_to_first_change();
    }

    /// The reverse of `enter_diff_preview_representation`: `R` from the `Preview` representation
    /// back to the diff's own `Source` (not whatever `[ui] diff_view` configures — the design's own
    /// explicit choice: "`R`＝Source の diff に戻る", not a fresh open). Reuses `open_git_diff` for
    /// every other bit of state it resets (the windowed reader, `md_cache`, the raw-diff cache, ...)
    /// and overrides just the two fields that call would otherwise get wrong: the presentation
    /// (`Source`, not the config default) and `came_from_git_view`/`diff_follow_scope` (which
    /// `open_git_diff` always resets — preserved here exactly like `diff_jump_changed` already
    /// preserves them around its own `open_git_diff` call, for the identical reason: this is a
    /// *return*, not a fresh open).
    #[cfg(feature = "git")]
    fn return_to_diff_from_preview(&mut self) {
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let came_from_git_view = self.tab.came_from_git_view;
        let diff_follow_scope = self.diff_follow_scope;
        self.open_git_diff(&path);
        self.tab.diff_view = DiffView::Source;
        self.tab.came_from_git_view = came_from_git_view;
        self.diff_follow_scope = diff_follow_scope;
    }
}
