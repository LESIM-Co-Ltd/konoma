//! File decisions ride the existing media mailbox. UI consumers only inspect adopted values.
use super::*;
use konoma_plugin::host::file_preview::{FilePreviewPlan, ResolvedFilePreview};
use konoma_plugin::Verb;

pub struct FilePreviewReply {
    kind: Reply,
}

enum Reply {
    Selected {
        serial: u64,
        path: PathBuf,
        id: Option<String>,
    },
    Diff {
        serial: u64,
        path: PathBuf,
        resolved: ResolvedFilePreview,
        exists: bool,
    },
    Preview {
        generation: u64,
        path: PathBuf,
        plan: FilePreviewPlan,
    },
}

pub(super) struct PendingFileVerb {
    serial: u64,
    path: PathBuf,
    verb: Verb,
    line: Option<usize>,
}

fn deliver(tx: &std::sync::mpsc::Sender<MediaResult>, kind: Reply) {
    let _ = tx.send(MediaResult {
        gen: 0,
        payload: Some(MediaPayload::FilePreview(Box::new(FilePreviewReply {
            kind,
        }))),
    });
}

impl App {
    pub(crate) fn is_file_preview_resolving(&self) -> bool {
        self.preview_resolution_tx.is_some()
            && self.media_loading
            && matches!(
                self.tab.preview_kind,
                Some(PreviewKind::CanNotPreview { .. })
            )
    }
    /// Attach before the terminal loop, using the same mailbox as plugin and media workers.
    /// Unattached hosts retain the existing ordinary-screen/headless preparation contract.
    pub fn attach_preview_resolution_worker(&mut self, tx: std::sync::mpsc::Sender<MediaResult>) {
        self.preview_resolution_tx = Some(tx);
        self.verb_plugin_memo.borrow_mut().take();
        self.refresh_diff_target_kind_cache();
    }

    pub(super) fn resolve_verb_plugin_on_worker(
        &self,
        path: &Path,
        settings: String,
    ) -> Option<String> {
        if let Some(memo) = self.verb_plugin_memo.borrow().as_ref() {
            if memo.path == path && memo.settings == settings {
                return memo.id.clone();
            }
        }
        let serial = self.preview_resolution_serial.get().wrapping_add(1);
        self.preview_resolution_serial.set(serial);
        *self.verb_plugin_memo.borrow_mut() = Some(open_app::VerbPluginMemo {
            path: path.into(),
            stamp: None,
            settings,
            id: None,
            request: Some(serial),
        });
        let tx = self.preview_resolution_tx.clone()?;
        let path = path.to_path_buf();
        let rules = self.cfg.preview.rules.clone();
        let disabled = self.cfg.plugin_disabled();
        let _ = std::thread::Builder::new()
            .name("file-preview-verbs".into())
            .spawn(move || {
                let id = crate::plugins::plugin_id_of(&rules, &disabled, &path);
                deliver(&tx, Reply::Selected { serial, path, id });
            });
        None
    }

    pub(super) fn resolve_diff_on_worker(&self, path: PathBuf) {
        let Some(tx) = self.preview_resolution_tx.clone() else {
            return;
        };
        let serial = self.diff_resolution_serial;
        let rules = self.cfg.preview.rules.clone();
        let disabled = self.cfg.plugin_disabled();
        let commands = self.cfg.external.preview_commands;
        let _ = std::thread::Builder::new()
            .name("file-preview-diff".into())
            .spawn(move || {
                let resolved =
                    crate::plugins::resolve_file(&rules, &disabled, commands, &path, None);
                let exists = path.exists();
                deliver(
                    &tx,
                    Reply::Diff {
                        serial,
                        path,
                        resolved,
                        exists,
                    },
                );
            });
    }

    pub(super) fn resolve_preview_on_worker(&mut self, path: &Path) {
        let Some(tx) = self.preview_resolution_tx.clone() else {
            return;
        };
        self.media_loading = true;
        let generation = self.media_gen;
        let path = path.to_path_buf();
        let rules = self.cfg.preview.rules.clone();
        let disabled = self.cfg.plugin_disabled();
        let commands = self.cfg.external.preview_commands;
        let _ = std::thread::Builder::new()
            .name("file-preview-plan".into())
            .spawn(move || {
                let plan = if crate::preview::is_previewable(&path) {
                    let source = konoma_plugin::host::file_preview::FilePreviewSource::new(
                        crate::plugins::shared_registry(&disabled),
                        rules.iter().map(Into::into).collect(),
                        commands,
                    );
                    source.plan(&path, None)
                } else {
                    FilePreviewPlan::Unavailable
                };
                deliver(
                    &tx,
                    Reply::Preview {
                        generation,
                        path,
                        plan,
                    },
                );
            });
    }

    pub(super) fn defer_file_verb(&mut self, verb: Verb, path: &Path, line: Option<usize>) -> bool {
        if self.preview_resolution_tx.is_none() {
            return false;
        }
        self.verb_plugin_id(path);
        let serial = self
            .verb_plugin_memo
            .borrow()
            .as_ref()
            .filter(|m| m.path == path)
            .and_then(|m| m.request);
        if let Some(serial) = serial {
            self.pending_file_verbs.push(PendingFileVerb {
                serial,
                path: path.into(),
                verb,
                line,
            });
            return true;
        }
        false
    }

    pub(super) fn apply_file_preview_reply(&mut self, reply: FilePreviewReply) -> bool {
        match reply.kind {
            Reply::Selected { serial, path, id } => {
                let mut changed = false;
                if let Some(memo) = self.verb_plugin_memo.borrow_mut().as_mut() {
                    if memo.request == Some(serial) && memo.path == path {
                        memo.request = None;
                        memo.id = id.clone();
                        changed = true;
                    }
                }
                let mut retained = Vec::new();
                for pending in std::mem::take(&mut self.pending_file_verbs) {
                    if pending.serial == serial && pending.path == path {
                        self.apply_resolved_file_verb(pending, id.as_deref());
                        changed = true;
                    } else {
                        retained.push(pending);
                    }
                }
                self.pending_file_verbs = retained;
                changed
            }
            Reply::Diff {
                serial,
                path,
                resolved,
                exists,
            } => {
                if serial != self.diff_resolution_serial
                    || !matches!(&self.tab.preview_kind, Some(PreviewKind::GitDiff(p)) if p == &path)
                {
                    return false;
                }
                self.diff_target_kind_cache = Some((path.clone(), resolved, exists));
                let view = self.round_diff_view(self.tab.diff_view_choice, &path);
                #[cfg(feature = "git")]
                if view == DiffView::Preview {
                    self.enter_diff_preview_representation(&path);
                    return true;
                }
                self.apply_diff_view(view, &path);
                self.tab.diff_scroll_pending = (view == DiffView::Rendered).then_some(path);
                true
            }
            Reply::Preview {
                generation,
                path,
                plan,
            } => {
                if generation != self.media_gen
                    || self.tab.mode != Mode::Preview
                    || self.tab.preview_path.as_ref() != Some(&path)
                {
                    return false;
                }
                self.media_loading = false;
                let kind = crate::plugins::kind_of(plan, &path);
                self.apply_resolved_preview(kind, &path);
                true
            }
        }
    }

    fn apply_resolved_file_verb(&mut self, pending: PendingFileVerb, id: Option<&str>) {
        use office_open::{Launch, OfficeOs};
        let disabled = self.cfg.plugin_disabled();
        let declaration = match pending.verb {
            Verb::Open => Some(crate::plugins::open_verb_of(&disabled, id)),
            Verb::Edit => {
                id.and_then(|id| crate::plugins::declared_verb_of(&disabled, id, Verb::Edit))
            }
        };
        let launch = konoma_plugin::launch::resolve_launch(
            pending.verb,
            OfficeOs::current(),
            declaration.as_ref(),
            &pending.path,
            &self.launch_policy(&pending.path),
        );
        match &launch {
            Launch::Editor { .. } => {
                let line = konoma_plugin::host::launch::edit_line(&launch, pending.line);
                self.pending_edit = Some((pending.path, line));
            }
            Launch::Gui(attempts) => self.start_chain(attempts.clone()),
            Launch::Terminal(attempts) => {
                let line = konoma_plugin::host::launch::edit_line(&launch, pending.line);
                self.queue_terminal_chain(attempts, line, &pending.path);
            }
            Launch::Disabled(reason) => {
                self.flash = Some(konoma_plugin::host::launch::disabled_notice(
                    self.lang, *reason,
                ))
            }
        }
    }
}

#[cfg(test)]
#[path = "file_preview_resolution_tests.rs"]
mod tests;
