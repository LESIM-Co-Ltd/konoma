//! State inspection and publication waits for the test-only project driver.
use super::*;
use crate::app::project::ProjectIdleSnapshot;

#[derive(Default)]
pub(super) struct FrameFence {
    state: u64,
    drawn: u64,
}
impl FrameFence {
    pub(super) fn adopt(&mut self, snapshot: &ProjectIdleSnapshot) {
        self.state = snapshot.state_generation;
        self.drawn = snapshot.drawn_generation;
    }
    pub(super) fn changed(&mut self) {
        self.state += 1;
    }
    pub(super) fn drawn(&mut self) {
        self.drawn = self.state;
    }
}

impl ProjectLoopState {
    pub(super) fn publish_state_for_test(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) {
        if let Some(slot) = &prepared.test_observed {
            let nav = self.screen.as_ref().or(self.diff_return.as_ref());
            let content = self.content(info).or_else(|| {
                nav.and_then(|n| self.runtimes.get(&n.project)?.views.get(&n.frame()?.key))
                    .cloned()
            });
            let menu = self.menu.map(|m| match m {
                konoma_host_core::keymap::CoreMenu::Copy => "copy",
                konoma_host_core::keymap::CoreMenu::Actions => "actions",
                _ => "other",
            });
            let mut state = super::super::observe_project_state_for_test(
                nav,
                content.as_deref(),
                self.dialog.as_ref(),
                menu,
                prepared.notice.clone(),
            );
            state.return_to_preview = self.preview_project;
            state.diff = self
                .diff
                .as_ref()
                .filter(|d| !d.closed && !self.preview_project)
                .map(|d| {
                    d.snapshot_for_test(
                        self.diff_return.as_ref().map_or_else(Vec::new, |n| {
                            n.stack.iter().map(|f| f.key.screen.clone()).collect()
                        }),
                        info.viewport.cols.saturating_add(2),
                    )
                });
            if let Some(diff) = self
                .diff
                .as_ref()
                .filter(|d| !d.closed && !self.preview_project)
            {
                state.menu = diff.menu_for_test();
            }
            state.root =
                Some(nav.map_or_else(|| prepared.open_dir.clone(), |nav| nav.project.root.clone()));
            state.open_dir = Some(prepared.open_dir.clone());
            state.target = self.screen.as_ref().and_then(|nav| {
                nav.selected_target(
                    content.as_deref()?,
                    self.viewport,
                    prepared.settings.diff_layout,
                )
            });
            state.chip = nav
                .and_then(|nav| self.runtimes.get(&nav.project))
                .and_then(|runtime| runtime.chip.clone());
            *slot.lock().unwrap() = Some(state);
        }
    }

    pub(super) fn idle_snapshot(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        pictures: Option<&Pictures>,
        frame: &FrameFence,
    ) -> ProjectIdleSnapshot {
        let contexts = self.screen.as_ref().is_none_or(|nav| {
            nav.frame().is_none_or(|f| {
                self.runtimes.get(&nav.project).is_some_and(|r| {
                    r.views.contains_key(&f.key)
                        && !r.dirty.contains(&f.key)
                        && request(info, &f.key)
                            .is_some_and(|req| r.contexts.get(&f.key) == req.project.as_ref())
                })
            })
        });
        ProjectIdleSnapshot {
            owners: self
                .runtimes
                .values()
                .all(|r| !r.busy && r.pending.is_none() && r.pending_retry.is_none()),
            replay: !self.replay.is_replaying()
                && self.pending_acts.is_empty()
                && self
                    .screen
                    .as_ref()
                    .and_then(|n| n.frame())
                    .is_none_or(|f| f.pending.is_empty()),
            contexts,
            detection: !prepared.detection.pending(),
            watch: prepared.detection.watch_settled_for_test(),
            representations: self.file_preview.is_some()
                || self.diff.as_ref().is_none_or(|d| d.wake_in().is_none()),
            preview: self.preview_open.is_none()
                && (self.file_preview.is_none() || self.preview_project),
            pictures: pictures.is_none_or(|p| p.settled_for_test()),
            navigation: self.root_switch.is_none() && !self.launch.pending(),
            state_generation: frame.state,
            drawn_generation: frame.drawn,
        }
    }

    pub(super) fn wait_owners_for_test(&mut self) -> bool {
        let mut waited = false;
        for runtime in self
            .runtimes
            .values_mut()
            .filter(|r| r.busy && r.pending.is_none())
        {
            runtime.pending = Some(
                runtime
                    .rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("project idle fence: owners (reply for requested context)"),
            );
            waited = true;
        }
        waited
    }

    pub(super) fn wait_idle_for_test(
        &mut self,
        prepared: &PreparedProjects,
        snapshot: &ProjectIdleSnapshot,
        mut pictures: Option<&mut Pictures>,
    ) {
        let waited = self.wait_owners_for_test();
        if !waited && !snapshot.contexts {
            if let Some(runtime) = self
                .screen
                .as_ref()
                .and_then(|nav| self.runtimes.get_mut(&nav.project))
            {
                if runtime.pending.is_none() {
                    runtime.pending = Some(
                        runtime
                            .rx
                            .recv_timeout(Duration::from_secs(30))
                            .unwrap_or_else(|e| {
                                panic!("project idle fence: requested context: {snapshot:?}: {e}")
                            }),
                    );
                }
            }
        }
        if !snapshot.detection {
            prepared.detection.wait_for_result();
        }
        if !snapshot.representations {
            if let Some(diff) = self.diff.as_mut() {
                diff.settle_representations_with_pictures_for_test(
                    pictures.as_deref_mut().map(Pictures::doc_pics),
                );
                if diff.data.is_some() && self.diff_return.is_none() {
                    self.diff_return = self.screen.take();
                }
            }
        }
        if let Some(pending) = self.preview_open.as_mut() {
            pending.fence_for_test();
        }
        if let Some(pictures) = pictures {
            pictures.settle_for_test();
        }
        if let Some(root) = self.root_switch.as_mut() {
            root.fence_for_test();
        }
        if !snapshot.navigation {
            self.launch.fence_for_test();
        }
    }
}
