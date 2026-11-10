//! Host value capture, terminal materialization and presentation of shared media diffs.
use super::*;
#[cfg(test)]
use crate::preview::media_diff::MediaDiffSide as KeySide;
use konoma_plugin::host::project::media_diff as worker;
use worker::is_jj;
pub(crate) use worker::media_format;
#[cfg(all(test, feature = "git"))]
use worker::resolve_old_bytes;
#[cfg(test)]
use worker::MediaDiffSideDecoded;
#[cfg(test)]
use worker::MediaDiffWorker;

impl App {
    pub fn attach_media_diff_loader(&mut self, tx: std::sync::mpsc::Sender<MediaDiffResult>) {
        self.media_diff.attach(tx);
    }

    fn media_diff_baseline(&self, path: &Path) -> DiffBaseline {
        self.diff_baseline(path, MdDiffKind::Rendered)
    }

    fn media_diff_landed_for(
        &self,
        path: &Path,
        page: u32,
        raster_px: (u32, u32),
    ) -> Option<MediaDiffOutcome> {
        self.media_diff.landed_for(path, page, raster_px)
    }

    pub(crate) fn poll_media_diff(
        &mut self,
        path: &Path,
        page: u32,
        raster_px: (u32, u32),
    ) -> Option<MediaDiffOutcome> {
        let raster_px = self.normalize_raster_px(path, raster_px);
        if self.media_diff.needs(path, page, raster_px) {
            self.kick_media_diff(path.to_path_buf(), page, raster_px);
        }
        self.media_diff_landed_for(path, page, raster_px)
    }

    fn normalize_raster_px(&self, path: &Path, raster_px: (u32, u32)) -> (u32, u32) {
        let kind = match self.media_landed_outcome_for(path) {
            Some(MediaDiffOutcome::Ready { kind, .. }) => Some(*kind),
            _ if self.preview_resolution_tx.is_none() => {
                classify_kind(&self.diff_target_kind(path))
            }
            _ => self
                .diff_target_kind_cache
                .as_ref()
                .filter(|(p, _, _)| p == path)
                .and_then(|(_, resolved, _)| resolved.media_kind),
        };
        worker::normalize_raster_px(kind, raster_px)
    }

    fn kick_media_diff(&mut self, path: PathBuf, page: u32, raster_px: (u32, u32)) {
        if let Some(want) = self.media_diff.kick((path, page, raster_px)) {
            self.dispatch_media_diff(want);
        }
    }

    fn dispatch_media_diff(&mut self, want: worker::MediaDiffWant) {
        #[cfg(test)]
        crate::test_support::note_media_diff_dispatch_call();
        let (path, page, raster_px) = want;
        let baseline = self.media_diff_baseline(&path);
        self.spawn_or_sync_media_diff(MediaDiffRequest {
            gen: 0,
            path,
            root: self.tab.root.clone(),
            baseline,
            page,
            raster_px,
            classify: media_classifier(
                self.cfg.preview.rules.clone(),
                self.cfg.plugin_disabled(),
                self.cfg.external.preview_commands,
            ),
            rasterize_svg: crate::preview::svg::rasterize_untrusted,
            cancel: Arc::default(),
        });
    }

    fn spawn_or_sync_media_diff(&mut self, req: MediaDiffRequest) {
        self.media_diff =
            std::mem::take(&mut self.media_diff).with_read_context(self.tab.diff_read_context);
        if let Some(result) = self.media_diff.dispatch(req) {
            self.apply_media_diff(result);
        }
    }

    #[cfg(test)]
    fn compute_media_diff(req: &MediaDiffRequest) -> MediaDiffComputed {
        MediaDiffWorker::compute_media_diff(req)
    }
    #[cfg(test)]
    fn compute_media_diff_with_cap(req: &MediaDiffRequest, cap: u64) -> MediaDiffComputed {
        MediaDiffWorker::compute_media_diff_with_cap(req, cap)
    }

    pub fn apply_media_diff(&mut self, res: MediaDiffResult) -> bool {
        let mut worker = std::mem::take(&mut self.media_diff);
        let applied = worker.apply(res, |computed| {
            let outcome = self.materialize_media_diff(computed);
            worker::prune_media_diff_picture_cache(&mut self.md_images.cache, Some(&outcome));
            outcome
        });
        self.media_diff = worker;
        if let Some(next) = self.media_diff.take_queued() {
            self.dispatch_media_diff(next);
        }
        applied
    }

    /// Host cache ownership stays here; outcome conversion is shared with standalone hosts.
    fn materialize_media_diff(&mut self, computed: MediaDiffComputed) -> MediaDiffOutcome {
        worker::materialize_media_diff(computed, |p| self.store_media_picture(p))
    }

    /// Test-only: the picture an entry holds, and the same SVG drawn again as the application
    /// draws it again (sharp zoom, rebuilding evicted pixels) at the given size.
    #[cfg(test)]
    pub(crate) fn md_first_and_redrawn_for_test(
        &self,
        key: &Path,
        max_px: u32,
    ) -> Option<(image::DynamicImage, image::DynamicImage)> {
        let entry = self.md_images.cache.get(key)?;
        let first = (**entry.decoded.as_ref()?).clone();
        let svg = entry.svg.clone()?;
        let again = crate::app::redraw_svg(&svg, &entry.svg_trust, max_px).ok()?;
        Some((first, again))
    }

    #[cfg(test)]
    fn materialize_side(&mut self, side: MediaDiffSideDecoded) -> MediaDiffSide {
        worker::materialize_side(side, |p| self.store_media_picture(p))
    }

    fn store_media_picture(&mut self, p: worker::MediaDiffPictureDecoded) {
        // Pre-place before applying: stale results must never revive an evicted entry.
        self.md_images
            .cache
            .entry(p.cache_key.clone())
            .or_default()
            .svg_trust = match &p.svg_base {
            Some(base) => SvgTrust::File(base.clone()),
            None => SvgTrust::Embedded,
        };
        let request = self.begin_md_request(&p.cache_key);
        // Preserve the ordinary preview's pixels and transparency handling.
        self.apply_md_image(MdImageResult {
            path: p.cache_key,
            image: Ok(p.image),
            svg: p.svg,
            reraster: false,
            frames: p.frames,
            request,
        });
    }

    pub(crate) fn invalidate_media_diff(&mut self) {
        self.media_diff.invalidate();
    }

    /// Removes every `media-diff://` entry from `md_image_cache` unconditionally — never touches a
    /// mermaid/math key (`is_media_diff_url` alone gates the predicate). Called wherever the diff
    /// surface stops being able to ever repopulate them itself, which `App::apply_media_diff`'s own
    /// landing-triggered prune (`live_cache_keys`) can't cover since there's no further landing to
    /// piggyback on: `App::back_to_tree` (closing the diff back to the tree/Git hub — nothing will
    /// poll/land a media diff again until some unrelated one is opened), and
    /// `App::invalidate_diff_caches` whenever the tab's active view isn't a media-diff-capable
    /// `GitDiff` target (an in-place retarget to a non-media file, or a tab switch landing on a tab
    /// whose own target isn't a media diff).
    ///
    /// **The rule is unconditional on tab identity**: `media_diff_landed`/`_pending`/`md_image_cache`
    /// are all `App`-level, not per-tab, so switching to *any* other tab (`App::invalidate_diff_
    /// caches` runs on every tab switch, not only a genuine retarget) frees a still-open media diff's
    /// pixels exactly like `back_to_tree` does, and switching back re-kicks a fresh computation
    /// (`switching_to_another_tab_and_back_redraws_both_pictures` pins the re-kick half). Deliberate
    /// memory-vs-recompute behavior, not a bug: at most one tab's media-diff pixels are ever resident
    /// at a time.
    pub(crate) fn prune_media_diff_picture_cache(&mut self) {
        worker::prune_media_diff_picture_cache(&mut self.md_images.cache, None);
    }

    /// `PerTab::diff_media_page`, floored at `1` (a fresh tab/diff starts there; nothing should
    /// ever observe `0`, but this is the one place every reader goes through so that stays true
    /// regardless).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_media_page(&self) -> u32 {
        self.tab.diff_media_page.max(1)
    }

    /// The media diff's current layout (`[git] media_diff`, cycled by `s` —
    /// `App::cycle_media_diff_layout`) — read by `ui/preview.rs::render_gitdiff_media` to call
    /// `preview::media_diff::layout`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_layout(&self) -> MediaDiffLayout {
        self.media_diff_layout
    }

    /// Whether `key` currently has an entry in `md_image_cache` — `ui/preview.rs::render_gitdiff_
    /// media` uses this to detect a landed `Picture` whose pixels were evicted from underneath it and
    /// re-kick a fresh computation instead of silently drawing nothing. This genuinely happens:
    /// `App::enter_preview`'s file-switch clear fires whenever the *previous* preview target's path
    /// differs from the one being entered — landing on a *different* file's preview first (file
    /// paging, a Markdown link, a bookmark jump) evicts the still-resident `media-diff://` keys.
    /// `evict_md_image_cache_key_for_test` reproduces that directly for tests.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn md_image_cache_contains(&self, key: &Path) -> bool {
        self.md_images.cache.contains_key(key)
    }

    /// Test-only: remove `key` from `md_image_cache` directly, simulating the eviction
    /// `md_image_cache_contains`'s own doc comment describes (a different file's preview reusing
    /// the one shared cache) without needing to actually reproduce that specific keypress sequence.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn evict_md_image_cache_key_for_test(&mut self, key: &Path) {
        self.md_images.cache.remove(key);
    }

    /// Test-only: every `media-diff://` key currently in `md_image_cache` — lets a test discover the
    /// *real* cache keys a production render landed (depending on the render path's own raster
    /// target) rather than re-deriving them via a second `poll_media_diff` call, whose `raster_px`
    /// might mismatch and trigger an unrelated poll of its own.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn md_image_cache_media_diff_keys_for_test(&self) -> Vec<PathBuf> {
        self.md_images
            .cache
            .keys()
            .filter(|k| crate::preview::media_diff::is_media_diff_url(&k.to_string_lossy()))
            .cloned()
            .collect()
    }

    /// The terminal's font cell size in pixels, or `None` when there is no picker at all (a terminal
    /// with no graphics protocol, or images disabled) — `ui/preview.rs::render_gitdiff_media` degrades
    /// to the binary summary line in that case.
    pub(crate) fn picker_cell_px(&self) -> Option<(u32, u32)> {
        self.picker.as_ref().map(|p| {
            let f = p.font_size();
            (f.width as u32, f.height as u32)
        })
    }

    /// Whether the diff's `Rendered` presentation is currently the image/PDF/SVG side-by-side view
    /// (as opposed to decorated Markdown blocks) — `ui/preview.rs::render_gitdiff` reads this to pick
    /// `render_gitdiff_media` over `render_diff_rendered`.
    ///
    /// `Rendered` is reachable only for Markdown and the media kinds (pinned by
    /// `app::tests::diff_media_active_agrees_with_the_single_not_markdown_check_across_every_
    /// reachable_kind`), so "not Markdown" identifies the side-by-side view. `diff_media_capable` is
    /// deliberately *not* used here: a deleted, content-sniffed (no glob) image classifies as
    /// `CanNotPreview` until the worker lands, which `diff_media_capable` would wrongly read as "not
    /// media".
    pub(crate) fn diff_media_active(&self) -> bool {
        if !self.diff_rendered_active() {
            return false;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        !self.diff_target_kind(path).is_markdown()
    }

    /// `Config::resolve_preview(path)`, memoized in `diff_target_kind_cache` — the single read side of
    /// that cache (`App::refresh_diff_target_kind_cache` is the single write side). A cache hit
    /// (`path` matches whatever the diff surface is currently targeting) is the hot, per-frame path,
    /// costing nothing beyond a path comparison and clone. The terminal host stays unavailable
    /// until its worker replies; unattached ordinary-screen/headless preparation keeps its fallback.
    pub(super) fn diff_target_kind(&self, path: &Path) -> PreviewKind {
        match &self.diff_target_kind_cache {
            Some((p, resolved, _)) if p.as_path() == path => {
                crate::plugins::kind_of(resolved.plan.clone(), path)
            }
            _ if self.preview_resolution_tx.is_none() => self.cfg.resolve_preview(path),
            _ => PreviewKind::can_not_preview(path),
        }
    }

    /// Refreshes `diff_target_kind_cache` for whatever the diff surface is currently targeting
    /// (`self.tab.preview_kind`'s `GitDiff` path, if any) — called from `App::invalidate_diff_caches`,
    /// which runs both on a genuine retarget and on an invalidation-only refresh, so one call site
    /// covers both. Clears the cache outright when the diff surface isn't showing a `GitDiff` at all,
    /// so a stale entry can never outlive the target it was resolved for.
    pub(super) fn refresh_diff_target_kind_cache(&mut self) {
        self.diff_resolution_serial = self.diff_resolution_serial.wrapping_add(1);
        if self.preview_resolution_tx.is_some() {
            self.diff_target_kind_cache = None;
            self.verb_plugin_memo.borrow_mut().take();
            if let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.clone() {
                self.resolve_diff_on_worker(path);
            }
            return;
        }
        self.diff_target_kind_cache = match self.tab.preview_kind.clone() {
            Some(PreviewKind::GitDiff(path)) => {
                let resolved = crate::plugins::resolve_file(
                    &self.cfg.preview.rules,
                    &self.cfg.plugin_disabled(),
                    self.cfg.external.preview_commands,
                    &path,
                    None,
                );
                let exists = path.exists();
                Some((path, resolved, exists))
            }
            Some(
                PreviewKind::Plugin { .. }
                | PreviewKind::Text(_)
                | PreviewKind::MermaidFence(_)
                | PreviewKind::Command { .. }
                | PreviewKind::CanNotPreview { .. },
            )
            | None => None,
        };
    }

    /// Whether `path` resolves to a media-diff-capable kind (Image/Svg/Pdf) — the one predicate
    /// `classify_kind` and `App::follow_jump`'s own `media_side_by_side` check both defer to, so the
    /// two can never drift apart on which kinds get the side-by-side treatment. Goes through the same
    /// memoized `diff_target_kind` as every other diff-surface predicate in this module.
    pub(crate) fn diff_media_capable(&self, path: &Path) -> bool {
        // Follow asks about a candidate before opening a diff. Its synchronous boolean filter
        // has no Pending result; preserve that controller's contract until navigation can await it.
        let active_diff =
            matches!(&self.tab.preview_kind, Some(PreviewKind::GitDiff(p)) if p == path);
        if self.preview_resolution_tx.is_none() || !active_diff {
            let kind = if active_diff {
                self.diff_target_kind(path)
            } else {
                self.cfg.resolve_preview(path)
            };
            return classify_kind(&kind).is_some();
        }
        self.diff_target_kind_cache
            .as_ref()
            .is_some_and(|(p, resolved, _)| p == path && resolved.media_kind.is_some())
    }

    /// Whether `path`'s Source-representation diff, if the raw line diff comes back **empty**,
    /// should be checked against this module's worker instead of being trusted at face value as
    /// "(no changes)". True for exactly the kinds with no text/decorated representation of their own
    /// to fall back on (video/archive/table/unsupported/an image-mode delegated command): these rely
    /// on git's raw line diff alone, which comes back empty for *any* binary file whether or not it
    /// actually changed, so an empty diff there is never proof of "no changes". Markdown/Code/Text/
    /// Mermaid/a text-mode command are excluded — an empty diff there is always genuinely "no
    /// changes". Image/Svg/Pdf aren't excluded, but in practice never reach `Source` at all
    /// (`App::round_diff_view` always substitutes `Rendered`) — if a test forces the state anyway,
    /// their kind resolves to `Some(_)` under `classify_kind`, landing on the worker's real `Ready`
    /// outcome rather than `Summary`, which the caller handles correctly either way.
    pub(crate) fn diff_binary_summary_eligible(&self, path: &Path) -> bool {
        let resolved = self.diff_target_kind(path);
        let windowed_text = matches!(resolved, PreviewKind::Text(_))
            || resolved.is_markdown()
            || resolved.is_file_text()
            || resolved.is_mermaid_file()
            || matches!(&resolved, PreviewKind::Command { render_as, .. } if render_as.as_deref() != Some("image"));
        !windowed_text
    }

    /// The landed media-diff outcome for `path`, ignoring the page/raster it was computed at (kind
    /// classification and page-count don't depend on either) — used by `App::diff_representations` to
    /// classify a **deleted** file `resolve_preview` can't, and by the page-turn/footer helpers below,
    /// called from contexts that don't have a raster target on hand at all.
    pub(super) fn media_landed_outcome_for(&self, path: &Path) -> Option<&MediaDiffOutcome> {
        self.media_diff.outcome_for(path)
    }

    fn active_media_outcome(&self) -> Option<&MediaDiffOutcome> {
        let PreviewKind::GitDiff(path) = self.tab.preview_kind.as_ref()? else {
            return None;
        };
        self.media_landed_outcome_for(path)
    }

    /// Whether the media diff currently on screen has a PDF side with ≥2 pages on either side — gates
    /// the `J`/`K` key and its footer/help hint.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_can_page(&self) -> bool {
        worker::can_page(self.active_media_outcome())
    }

    /// Whether the media diff's side-by-side view isn't just *targeted* (`diff_media_active` — true
    /// the instant the diff opens, before anything has landed) but is actually **showing at least one
    /// picture side by side right now**: the landed outcome is `Ready` and at least one side is a
    /// `Picture`. Gates the `s` key and its footer/help hint ([[hint-shown-iff-key-acts]]): while the
    /// worker is still computing, or the outcome degraded to the binary summary, or both sides landed
    /// as `Absent`/`Failed`/`PageMissing`, there is no layout for `s` to cycle between.
    /// `media_diff_can_page` derives its own narrower gate from the same shared ready sides —
    /// this is that fn's sibling for `s`, which only needs *a* picture, not a multi-page one.
    pub(crate) fn media_diff_showing_pictures(&self) -> bool {
        self.diff_media_active() && worker::showing_pictures(self.active_media_outcome())
    }

    /// `J`/`K` (and `PageDown`/`PageUp` while the media diff's side-by-side view is active): turn both
    /// sides' PDF page together, clamped to the larger of the two sides' own page counts. No-op for
    /// anything but a multi-page PDF diff — re-derived here rather than trusted from the caller, so a
    /// keypress queued from a frame where paging *was* available can't wrap past a since-shrunk page
    /// count (e.g. `n`/`N` landed on a single-page file in between).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_page_turn(&mut self, dir: i32) {
        let next = worker::page_turn(self.active_media_outcome(), self.diff_media_page(), dir);
        if next != self.diff_media_page() {
            self.tab.diff_media_page = next;
        }
    }

    /// `s` while the media diff's side-by-side view is active: auto → side → stack → auto. Flashes
    /// the layout it switched to, mirroring `App::cycle_diff_layout`'s own flash for the text diff's
    /// `s`.
    pub(crate) fn cycle_media_diff_layout(&mut self) {
        self.media_diff_layout = self.media_diff_layout.next();
        let label = crate::i18n::tr(self.lang, media_layout_msg(self.media_diff_layout));
        self.flash = Some(label.into());
    }

    /// The `Msg` naming what `s` would switch the media diff's layout **to** — the footer's dynamic
    /// `s:<next>` fragment, mirroring `R`'s own `diff_view_cycle_hint`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_layout_next_msg(&self) -> crate::i18n::Msg {
        media_layout_msg(self.media_diff_layout.next())
    }

    /// Whether the GitDiff footer (`ui/status.rs::mode_footer`) should show the reduced media/
    /// binary-summary hint set (`n/N`, `x`(write), `q/Esc` — plus `s`/`J`/`K`/`R` inserted dynamically
    /// only while the side-by-side view is active) instead of the classic scrollable-diff hint set.
    /// True for the media diff's own side-by-side view (`diff_media_active`) and for any
    /// binary-summary-eligible kind's `Source` representation (`diff_binary_summary_eligible`) — the
    /// latter's raw line diff is *always* empty (git/jj never line-diff a binary file), so — unlike
    /// `diff_rendered_active`'s own gate — this needs no per-frame "is the diff actually empty right
    /// now" check (which would need `&mut self`, unavailable to the footer) to be correct.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_footer_is_media_or_summary(&self) -> bool {
        if self.diff_media_active() {
            return true;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        self.diff_binary_summary_eligible(path)
    }

    /// The `Msg` naming a media diff's **old**-side base (`MediaBase`, from the landed outcome) — the
    /// caption label.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_base_msg(base: MediaBase) -> crate::i18n::Msg {
        match base {
            MediaBase::Head => crate::i18n::Msg::MediaBaseHead,
            MediaBase::JjParent => crate::i18n::Msg::MediaBaseJjParent,
            MediaBase::FollowStart => crate::i18n::Msg::MediaBaseFollowStart,
        }
    }

    /// The `Msg` naming a media diff's **new** side — always the live working copy (never a follow
    /// snapshot): git's "working tree" or jj's "working copy (@)", decided the same way
    /// `resolve_old_bytes` decides the *old* side's label.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_new_side_msg(&self) -> crate::i18n::Msg {
        if is_jj(&self.tab.root) {
            crate::i18n::Msg::MediaBaseWorkCopyJj
        } else {
            crate::i18n::Msg::MediaBaseWorkTreeGit
        }
    }
}

/// Shared by `App::cycle_media_diff_layout`'s flash and `App::media_diff_layout_next_msg`'s footer
/// hint, so the two can never name the layout differently.
fn media_layout_msg(l: MediaDiffLayout) -> crate::i18n::Msg {
    match l {
        MediaDiffLayout::Auto => crate::i18n::Msg::MediaLayoutAuto,
        MediaDiffLayout::Side => crate::i18n::Msg::MediaLayoutSide,
        MediaDiffLayout::Stack => crate::i18n::Msg::MediaLayoutStack,
    }
}

pub(super) fn classify_kind(kind: &PreviewKind) -> Option<MediaDiffKind> {
    classify_kind_with(kind, None)
}

/// [`classify_kind`] with the bytes of the file at hand (the worker has them, also for a file that
/// is gone and was read from the VCS).
fn classify_kind_with(kind: &PreviewKind, bytes: Option<&[u8]>) -> Option<MediaDiffKind> {
    match kind {
        PreviewKind::Plugin { path, .. } => {
            worker::classify_kind_with(media_target(kind), path, bytes)
        }
        PreviewKind::Text(_)
        | PreviewKind::GitDiff(_)
        | PreviewKind::MermaidFence(_)
        | PreviewKind::Command { .. }
        | PreviewKind::CanNotPreview { .. } => None,
    }
}

fn media_classifier(
    rules: Vec<crate::config::Rule>,
    disabled: Vec<String>,
    commands: bool,
) -> Arc<worker::MediaClassifier> {
    Arc::new(move |path, bytes| {
        let kind = crate::config::resolve_preview_kind(&rules, &disabled, commands, path, bytes);
        media_target(&kind)
    })
}

/// The SDK receives this plain value; every host preview variant is named here.
fn media_target(kind: &PreviewKind) -> konoma_host_core::project::DiffTarget {
    match kind {
        PreviewKind::Plugin { .. } if kind.profile().image => {
            konoma_host_core::project::DiffTarget::Picture
        }
        PreviewKind::Plugin { .. }
        | PreviewKind::Text(_)
        | PreviewKind::GitDiff(_)
        | PreviewKind::MermaidFence(_)
        | PreviewKind::Command { .. }
        | PreviewKind::CanNotPreview { .. } => konoma_host_core::project::DiffTarget::Other,
    }
}

/// Whether the file's content is a video (`video/*`), judged from the file itself with the same
/// sniffer the host's `mime = "video/*"` rule uses. A file that cannot be read (a deleted one) has
/// no content to judge and is not one.
pub(super) fn is_video_file(path: &Path) -> bool {
    infer::get_from_path(path)
        .ok()
        .flatten()
        .is_some_and(|t| t.mime_type().starts_with("video/"))
}

#[cfg(test)]
#[path = "media_diff_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "media_diff_cache_bridge.rs"]
mod cache_bridge;

#[cfg(all(test, feature = "git"))]
#[path = "media_diff_screen_bridge.rs"]
mod screen_bridge;

#[cfg(all(test, feature = "git"))]
#[path = "media_diff_draw_tests.rs"]
mod draw_tests;
