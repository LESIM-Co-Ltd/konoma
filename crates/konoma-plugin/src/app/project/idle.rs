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
            preview: self.preview_open.is_none() && self.file_preview.is_none(),
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
