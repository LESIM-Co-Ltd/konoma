//! Selected-file classification on workers and launch delivery through the existing seams.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use konoma_host_core::project::{tool_disabled_notice, tool_failed_prefix};
use ratatui::{backend::Backend, Terminal};

use super::{PreparedProjects, ProjectBackend, ProjectLoopCx, ProjectServiceInfo, ProjectServices};
use crate::app::{AppError, Pictures};
use crate::host::project::launch::{project_tool_launch, ProjectToolLaunch};
use crate::launch::{Launch, Opener};
use crate::registry::Registry;
use crate::{Verb, VerbDecl};

/// The selected file or worktree directory, its source line (if any), and its declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLaunchTarget {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub verbs: Vec<VerbDecl>,
}

impl ProjectLaunchTarget {
    /// Read classification only on the worker, using the supplied service/registry adapter.
    pub fn spawn<S: ProjectServices + ?Sized>(
        path: PathBuf,
        line: Option<usize>,
        services: Arc<S>,
        registry: Arc<Registry>,
    ) -> mpsc::Receiver<Option<Self>> {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("project-launch".into())
            .spawn(move || {
                let target = path.exists().then(|| Self {
                    verbs: services.launch_verbs(&path, &registry),
                    path,
                    line,
                });
                let _ = tx.send(target);
            });
        rx
    }
}

pub(super) struct LaunchState {
    queue: VecDeque<PendingLaunch>,
    tx: mpsc::Sender<Result<Opener, String>>,
    rx: mpsc::Receiver<Result<Opener, String>>,
    launching: usize,
    refresh: bool,
}

enum PendingLaunch {
    File(Verb, mpsc::Receiver<Option<ProjectLaunchTarget>>),
    Tool(ProjectToolLaunch, bool),
    Copy(mpsc::Receiver<String>),
}

impl Default for LaunchState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            queue: VecDeque::new(),
            tx,
            rx,
            launching: 0,
            refresh: false,
        }
    }
}

impl LaunchState {
    pub(super) fn copy(&mut self, prepared: &PreparedProjects, text: String) {
        let lang = prepared.keys.lang();
        #[cfg(any(test, feature = "test-hooks"))]
        let sink = prepared.test_clipboard.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let write = || konoma_host_core::clipboard::set_text(&text).map_err(|e| e.to_string());
            #[cfg(any(test, feature = "test-hooks"))]
            let outcome = sink.map_or_else(write, |sink| sink.write(&text));
            #[cfg(not(any(test, feature = "test-hooks")))]
            let outcome = write();
            let notice = match outcome {
                Ok(()) => konoma_host_core::copy::copied_preview_notice(lang, &text),
                Err(error) => konoma_host_core::copy::copy_failed_notice(lang, &error),
            };
            let _ = tx.send(notice);
        });
        self.queue.push_back(PendingLaunch::Copy(rx));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn fence_for_test(&mut self) {
        if let Some(PendingLaunch::Copy(reply)) = self.queue.front_mut() {
            let notice = reply
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("project idle fence: copy publication");
            let (tx, rx) = mpsc::channel();
            tx.send(notice).expect("copy receiver");
            *reply = rx;
        }
        if let Some(PendingLaunch::File(_, reply)) = self.queue.front_mut() {
            let target = reply
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("project idle fence: launch target publication");
            let (tx, rx) = mpsc::channel();
            tx.send(target).expect("launch target receiver");
            *reply = rx;
        }
        if self.launching > 0 {
            let result = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("project idle fence: launch reply publication");
            self.tx.send(result).expect("launch receiver");
        }
    }
    pub(super) fn file(
        &mut self,
        prepared: &PreparedProjects,
        verb: Verb,
        path: PathBuf,
        line: Option<usize>,
    ) {
        if let Some(source) = prepared.file_previews.clone() {
            let (tx, rx) = mpsc::channel();
            let _ = std::thread::Builder::new()
                .name("project-preview-launch".into())
                .spawn(move || {
                    let target = path.exists().then(|| {
                        let resolved = source.resolve(&path, None);
                        ProjectLaunchTarget {
                            verbs: source.verbs(&resolved),
                            path,
                            line,
                        }
                    });
                    let _ = tx.send(target);
                });
            self.queue.push_back(PendingLaunch::File(verb, rx));
            return;
        }
        self.queue.push_back(PendingLaunch::File(
            verb,
            prepared
                .services
                .clone()
                .spawn_launch_target(path, line, prepared.registry.clone()),
        ));
    }

    pub(super) fn tool(&mut self, prepared: &mut PreparedProjects, info: &ProjectServiceInfo) {
        if !prepared.settings.tool_enabled {
            prepared.notice = Some(tool_disabled_notice(prepared.keys.lang()).into());
            return;
        }
        let jj = info.backend == ProjectBackend::Jj;
        if let Some(plan) =
            project_tool_launch(&prepared.settings.tool_command(jj).0, &info.workdir)
        {
            self.queue.push_back(PendingLaunch::Tool(plan, jj));
        }
    }

    pub(super) fn pending(&self) -> bool {
        !self.queue.is_empty() || self.launching > 0
    }

    pub(super) fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh)
    }

    pub(super) fn poll<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        prepared: &mut PreparedProjects,
        cx: &ProjectLoopCx,
        pictures: Option<&mut Pictures>,
    ) -> Result<bool, AppError> {
        if let Some(PendingLaunch::Copy(reply)) = self.queue.front() {
            if let Ok(notice) = reply.try_recv() {
                self.queue.pop_front();
                prepared.notice = Some(notice);
                return Ok(true);
            }
        }
        let Some(lc) = cx.launch.as_ref() else {
            return Ok(false);
        };
        let mut changed = false;
        while let Ok(outcome) = self.rx.try_recv() {
            self.launching = self.launching.saturating_sub(1);
            prepared.notice = Some(crate::host::launch::chain_notice(
                prepared.keys.lang(),
                &outcome,
            ));
            changed = true;
        }
        let target = match self.queue.front() {
            Some(PendingLaunch::File(verb, rx)) => match rx.try_recv() {
                Ok(target) => Some((*verb, target)),
                Err(mpsc::TryRecvError::Disconnected) => Some((*verb, None)),
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if target.is_some() {
            self.queue.pop_front();
        }
        // Invalidating ratatui's previous frame is required even if a failed seam never calls
        // redraw. The next loop frame includes the failure notice and all restored pictures.
        let mut redraw = || {
            terminal
                .clear()
                .map_err(|e| std::io::Error::other(e.to_string()))
        };
        let mut handed = false;
        if let Some((verb, None)) = target {
            use konoma_host_core::help_msg::CoreMsg;
            prepared.notice = Some(
                if verb == Verb::Edit {
                    CoreMsg::NoFileToEdit
                } else {
                    CoreMsg::NoFileToOpenApp
                }
                .tr(prepared.keys.lang())
                .into(),
            );
            changed = true;
        }
        if let Some((verb, Some(target))) = target {
            let facts = lc.facts(&target.verbs, &target.path);
            if (verb == Verb::Edit && !facts.edit_key()) || (verb == Verb::Open && !facts.open_key)
            {
                return Ok(changed);
            }
            let resolved = lc.resolve(&target.verbs, verb, &target.path);
            let gui = matches!(resolved, Launch::Gui(_));
            // The file app retains its existing editor exit policy. Project launches also
            // report a non-zero start, without duplicating edit/open resolution or execution.
            let mut checked = lc.clone();
            let hand_off = checked.hand_off.clone();
            if matches!(resolved, Launch::Editor { .. }) {
                checked.hand_off = Arc::new(move |a, cwd, redraw| {
                    let status = hand_off(a, cwd, redraw)?;
                    match status {
                        Some(code) if code != 0 => Err(std::io::Error::other(format!(
                            "{} exited with status {code}",
                            a.prog
                        ))),
                        _ => Ok(status),
                    }
                });
            }
            let done = if verb == Verb::Edit {
                crate::app::launch::edit(
                    &checked,
                    &target.verbs,
                    &target.path,
                    target.line,
                    prepared.keys.lang(),
                    &self.tx,
                    &mut redraw,
                )
            } else {
                crate::app::launch::open(
                    &checked,
                    &target.verbs,
                    &target.path,
                    prepared.keys.lang(),
                    &self.tx,
                    &mut redraw,
                )
            };
            if gui && lc.worker {
                self.launching += 1;
            }
            if done.notice.is_some() {
                prepared.notice = done.notice;
            }
            handed = done.handed;
            self.refresh |= done.reload;
            changed = true;
        }
        if matches!(self.queue.front(), Some(PendingLaunch::Tool(..))) {
            let Some(PendingLaunch::Tool(plan, jj)) = self.queue.pop_front() else {
                unreachable!()
            };
            let result = (lc.hand_off)(&plan.attempt, &plan.cwd, &mut redraw);
            let failure = match result {
                Err(e) => Some(e.to_string()),
                Ok(Some(code)) if code != 0 => Some(format!("exited with status {code}")),
                _ => None,
            };
            if let Some(error) = failure {
                prepared.notice = Some(format!(
                    "{}{}: {error}",
                    tool_failed_prefix(prepared.keys.lang(), jj),
                    plan.attempt.prog
                ));
            }
            handed = true;
            self.refresh = true;
            changed = true;
        }
        if handed {
            terminal
                .clear()
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            if let Some(p) = pictures {
                // What was drawn went with the terminal; the user's view of the still picture
                // waits for it to be drawn again (the Preview opened again after the hand-off is
                // put in it, as a konoma tab keeps its `image_view` over the reload).
                let pose = p.still_pose();
                p.forget();
                p.forget_still();
                p.reset_document();
                if let Some(pose) = pose {
                    p.restore_still_pose(pose);
                }
            }
        }
        Ok(changed)
    }
}
