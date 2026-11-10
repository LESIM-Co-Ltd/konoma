//! Candidate preparation owns fresh detection and mailboxes until its first view succeeds.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::*;

pub(in crate::app::project) struct PendingRootSwitch {
    reply: Receiver<Result<PreparedProjectRoot, String>>,
}

struct PreparedProjectRoot {
    prepared: PreparedProjects,
    state: ProjectLoopState,
}

impl ProjectLoopState {
    pub(super) fn start_root_switch(
        &mut self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        root: PathBuf,
        message: Option<crate::Tr>,
    ) {
        let registry = prepared.registry.clone();
        let file_previews = prepared.file_previews.clone();
        let config = prepared.config.clone();
        let services = prepared.services.clone();
        let settings = prepared.settings.clone();
        let term = prepared.term;
        let lang = prepared.lang;
        let info = info.clone();
        let launch_enabled = self.launch_enabled;
        #[cfg(feature = "test-hooks")]
        let fence = prepared.root_switch_fence.clone();
        let (tx, reply) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                // Match konoma's navigation refusal before attempting project preparation.
                if !root.is_dir() {
                    return Err(format!(
                        "{}{}",
                        if lang == "ja" {
                            "見つかりません: "
                        } else {
                            "not found: "
                        },
                        root.display()
                    ));
                }
                let mut prepared = super::super::prepare::prepare_registered(
                    registry,
                    &root,
                    term,
                    config,
                    services,
                    ProjectDetection::default(),
                )
                .map_err(|e| e.to_string())?;
                prepared.settings = settings;
                prepared.file_previews = file_previews;
                let mut state = ProjectLoopState::new(&mut prepared, launch_enabled);
                state.viewport = info.viewport;
                let cx = ProjectLoopCx {
                    home: info.home,
                    launch: None,
                };
                let current = state.info(&prepared, &cx);
                // Adopt open replies through the same host facade as the live loop. All
                // blocking receives here are on this preparation worker, never on the UI.
                state.prepare_first_screen(&mut prepared, &current)?;
                prepared.notice = message.map(|m| m.get(lang).into()).or_else(|| {
                    Some(format!(
                        "{}: {}",
                        if lang == "ja" {
                            "切替"
                        } else {
                            "Switched to"
                        },
                        konoma_host_core::paths::home_relative_to(
                            &prepared.open_dir,
                            cx.home.as_deref()
                        )
                    ))
                });
                #[cfg(feature = "test-hooks")]
                {
                    prepared.root_switch_fence = fence.clone();
                    if let Some(fence) = &fence {
                        for runtime in state.runtimes.values_mut() {
                            let (tx, rx) = mpsc::sync_channel(1);
                            let owner = std::mem::replace(&mut runtime.rx, rx);
                            let fence = fence.clone();
                            std::thread::spawn(move || {
                                while let Ok(reply) = owner.recv() {
                                    if tx.send(reply).is_err() {
                                        break;
                                    }
                                    let _ = fence.send(());
                                }
                            });
                        }
                    }
                }
                Ok(PreparedProjectRoot { prepared, state })
            })();
            let _ = tx.send(result);
            #[cfg(feature = "test-hooks")]
            if let Some(fence) = fence {
                let _ = fence.send(());
            }
        });
        self.root_switch = Some(PendingRootSwitch { reply });
    }

    fn prepare_first_screen(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> Result<(), String> {
        loop {
            let retained: Vec<_> = self.runtimes.keys().cloned().collect();
            let update = self.with_host(info, |host, context| {
                host.poll(
                    std::slice::from_mut(context),
                    std::slice::from_ref(info),
                    &retained,
                )
            });
            if let Some(error) = self.runtimes.values().find_map(|r| r.error.as_ref()) {
                return Err(error.message.get(prepared.lang).into());
            }
            self.apply(prepared, info, update);
            self.ensure(info);
            if self.screen.is_none() || self.content(info).is_some() {
                return Ok(());
            }
            for runtime in self.runtimes.values_mut().filter(|r| r.busy) {
                runtime.pending = Some(runtime.rx.recv().map_err(|e| e.to_string())?);
            }
        }
    }

    pub(super) fn poll_root_switch(
        &mut self,
        prepared: &mut PreparedProjects,
        pictures: Option<&mut Pictures>,
    ) -> bool {
        let Some(pending) = &self.root_switch else {
            return false;
        };
        let result = match pending.reply.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(
                crate::t!(en: "project worker failed", ja: "プロジェクトの処理に失敗しました")
                    .get(prepared.lang)
                    .into(),
            ),
        };
        self.root_switch = None;
        match result {
            Ok(mut candidate) => {
                let old_info = self.info(prepared, &ProjectLoopCx::default());
                self.with_host(&old_info, |host, _| host.close_context(old_info.id));
                // Closing uses the existing serial channel. A busy owner is released when its
                // receiver disappears; no old mailbox is retained or polled after this swap.
                let mut old_state = std::mem::replace(self, candidate.state);
                // A background reaper can be arbitrarily delayed. Close the delivery boundary
                // now, while the old root's data and sessions are reclaimed off the UI thread.
                for runtime in old_state.runtimes.values_mut() {
                    runtime.disconnect();
                }
                candidate.prepared.load_error = prepared.load_error.take();
                let old_prepared = std::mem::replace(prepared, candidate.prepared);
                konoma_base::discard::discard_in_background((old_state, old_prepared));
                if let Some(p) = pictures {
                    p.forget();
                    p.forget_still();
                    p.reset_document();
                    p.doc_pics().prune_media_diff_picture_cache();
                }
            }
            Err(error) => prepared.notice = Some(error),
        }
        true
    }
}
