//! File opening belongs to a worker; presentation belongs to the stage 3 runner.
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::{PreparedProjects, ProjectLoopCx};
use crate::app::diff_screen::DiffScreenReturn;
use crate::app::looping::FileLoopState;
use crate::app::{AppActs, AppCx, AppError, Opened, ReopenSource};
use crate::host::file_preview::{FilePreviewPlan, FilePreviewSource};
use crate::{Input, Manifest, OpenCx, Session, View, ViewCx, ViewKind, ViewReq};

pub(super) struct FilePreviewState {
    pub(super) opened: Opened,
    pub(super) runner: FileLoopState,
    pub(super) source: DiffScreenReturn,
}

pub(super) struct PendingPreview {
    reply: Receiver<Result<Opened, AppError>>,
    source: DiffScreenReturn,
}

impl PendingPreview {
    pub(super) fn matches(&self, diff: &DiffScreenReturn) -> bool {
        self.source.path == diff.path && diff.choice == konoma_host_core::project::DiffView::Preview
    }
    pub(super) fn start(prepared: &PreparedProjects, source: DiffScreenReturn) -> Option<Self> {
        let previews = prepared.file_previews.clone()?;
        let config = prepared.config.clone();
        let term = prepared.term;
        let lang = prepared.lang;
        let path = source.path.clone();
        #[cfg(test)]
        let fence = prepared.preview_open_fence.clone();
        let (tx, reply) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("project-file-preview".into())
            .spawn(move || {
                let result = open(previews, path, &config, term, lang);
                let _ = tx.send(result);
                #[cfg(test)]
                if let Some(fence) = fence {
                    let _ = fence.send(());
                }
            });
        Some(Self { reply, source })
    }

    pub(super) fn poll(&self) -> Option<Result<Opened, AppError>> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(AppError::Open("file diff worker disconnected".into())))
            }
        }
    }

    pub(super) fn finish(
        self,
        opened: Opened,
        prepared: &PreparedProjects,
        cx: &ProjectLoopCx,
    ) -> FilePreviewState {
        let context = context(&opened, prepared, cx);
        FilePreviewState {
            runner: FileLoopState::new(&context, &prepared.keys),
            opened,
            source: self.source,
        }
    }
}

impl FilePreviewState {
    pub(super) fn context(&self, prepared: &PreparedProjects, cx: &ProjectLoopCx) -> AppCx {
        context(&self.opened, prepared, cx)
    }
}

fn context(opened: &Opened, prepared: &PreparedProjects, cx: &ProjectLoopCx) -> AppCx {
    let config = prepared.config.clone();
    let settings =
        move |id: &str| crate::config::plugin_settings(&config.plugins, &config.ui.moved(), id);
    let acts_config = prepared.config.clone();
    let mut reopen = ReopenSource::from_registry(opened.registry.clone(), Box::new(settings))
        .with_acts(move |m| {
            AppActs::from_config(&m.id, m, &acts_config.keys, &acts_config.plugins).0
        });
    if let Some(plugin) = opened.registry.plugin(&opened.plugin_id) {
        reopen = reopen.with_file(
            plugin,
            Input::File {
                path: opened.path.clone(),
                hint: opened.hint.clone(),
            },
            crate::config::plugin_settings(
                &prepared.config.plugins,
                &prepared.config.ui.moved(),
                &opened.plugin_id,
            ),
        );
    }
    let mut launch = cx.launch.clone();
    if let Some(launch) = &mut launch {
        launch.verbs = opened.manifest.verbs.clone();
    }
    AppCx {
        lang: prepared.lang.into(),
        term: prepared.term,
        title: opened.title.clone(),
        unavailable: opened.unavailable.clone(),
        plugin_id: opened.plugin_id.clone(),
        path: opened.path.clone(),
        hint: opened.hint.clone(),
        shows: opened.shows.clone(),
        reopen: Some(reopen),
        display: prepared.display.clone(),
        open_dir: Some(prepared.open_dir.clone()),
        acts: AppActs::from_config(
            &opened.plugin_id,
            &opened.manifest,
            &prepared.config.keys,
            &prepared.config.plugins,
        )
        .0,
        launch,
        converts: opened.manifest.converts,
    }
}

fn open(
    source: FilePreviewSource,
    path: PathBuf,
    config: &konoma_host_core::shared::SharedConfig,
    term: crate::TermCaps,
    lang: &str,
) -> Result<Opened, AppError> {
    let metadata = std::fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::NoFile(path.clone())
        } else {
            AppError::Unreadable(path.clone(), e)
        }
    })?;
    if !metadata.is_file() {
        return Err(AppError::NotAFile(path));
    }
    let registry = source.registry();
    let (manifest, hint, session): (_, _, Box<dyn Session>) = match source.plan(&path, None) {
        FilePreviewPlan::Plugin { id, hint } => {
            let plugin = registry
                .plugin(&id)
                .ok_or_else(|| AppError::Plugin(format!("plugin `{id}` is not registered")))?;
            let manifest = registry
                .manifest(&id)
                .cloned()
                .unwrap_or_else(|| plugin.manifest());
            let settings = crate::config::plugin_settings(&config.plugins, &config.ui.moved(), &id);
            let session = plugin
                .open(
                    Input::File {
                        path: path.clone(),
                        hint: hint.clone(),
                    },
                    &OpenCx::new(&settings, lang).with_term(term),
                )
                .map_err(|f| AppError::Open(f.message.get(lang).into()))?;
            (manifest, hint, session)
        }
        FilePreviewPlan::Text => {
            let bytes = std::fs::read(&path).map_err(|e| AppError::Unreadable(path.clone(), e))?;
            (
                Manifest::new("text", "0").shows(ViewKind::Text).file_text(),
                None,
                Box::new(Text(String::from_utf8_lossy(&bytes).into_owned())),
            )
        }
        _ => {
            return Err(AppError::NotHandled {
                path,
                why: "no plugin is available to show this document".into(),
            })
        }
    };
    Ok(Opened {
        title: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        plugin_id: manifest.id.clone(),
        unavailable: manifest.unavailable.clone(),
        shows: manifest.shows.clone(),
        manifest,
        hint,
        path,
        session,
        registry,
    })
}

struct Text(String);
impl Session for Text {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        Ok(View::Text(crate::view::TextView {
            source: crate::view::TextSource::Owned(self.0.clone()),
            styler: None,
            file_lines: true,
            syntax_name: None,
        }))
    }
}
