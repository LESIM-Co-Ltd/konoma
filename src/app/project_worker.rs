//! One owner thread per project. Neither session calls nor file/document rendering run on the UI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use konoma_plugin::project::{Chip, ProjectView};
use konoma_plugin::view::{TextSource, View, ViewReq};
use konoma_plugin::{
    ActCx, Failure, Input, OpenCx, Plugin, ReadScope, Session, Settings, TermCaps, Tr, ViewCx,
};

use super::project_screens::{ProjectContent, ProjectEffect, ProjectKey, ScreenKey};

pub(super) enum ProjectJob {
    Chip,
    View {
        key: ScreenKey,
        req: ViewReq,
    },
    Act {
        action: String,
        context: konoma_plugin::project::ProjectAct,
        refresh: Vec<(ScreenKey, ViewReq)>,
        chip: bool,
        effect: Box<ProjectEffect>,
    },
}

pub(super) struct ProjectReply {
    pub views: Vec<(ScreenKey, Arc<ProjectContent>)>,
    pub enabled: HashMap<String, bool>,
    pub chip: Option<Option<Chip>>,
    pub chip_error: Option<Failure>,
    pub effect: Option<ProjectEffect>,
    pub error: Option<Failure>,
}

impl ProjectReply {
    fn empty() -> Self {
        Self {
            views: Vec::new(),
            enabled: HashMap::new(),
            chip: None,
            chip_error: None,
            effect: None,
            error: None,
        }
    }
}

pub(crate) struct ProjectRuntime {
    pub(crate) project: crate::plugins::projects::MatchedProject,
    pub(super) views: HashMap<ScreenKey, Arc<ProjectContent>>,
    pub(super) enabled: HashMap<String, bool>,
    pub(super) chip: Option<Chip>,
    pub(super) chip_dirty: bool,
    pub(super) busy: bool,
    pub(super) acting: bool,
    #[cfg(test)]
    pub(super) view_request: Option<ScreenKey>,
    pub(super) dirty: std::collections::HashSet<ScreenKey>,
    pub(super) ready: bool,
    pub(crate) error: Option<Failure>,
    pub(super) tx: mpsc::SyncSender<ProjectJob>,
    pub(super) rx: mpsc::Receiver<ProjectReply>,
    pub(super) pending: Option<ProjectReply>,
    pub(super) invalidated: bool,
}

impl ProjectRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        project: crate::plugins::projects::MatchedProject,
        plugin: Arc<dyn Plugin>,
        actions: Vec<String>,
        settings: Settings,
        lang: String,
        theme: String,
        term: TermCaps,
        table: Option<toml::Table>,
        standard: bool,
        home: Option<PathBuf>,
        panic_message: String,
    ) -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        let root = project.root.clone();
        let marker = project.marker.clone();
        let chip = project
            .declaration
            .chip
            .as_ref()
            .map(|chip| Chip::new(chip.get(&lang)));
        let fallback = chip.clone();
        std::thread::spawn(move || {
            let scope = crate::plugins::read_scope::scope_for(
                standard,
                table.as_ref(),
                &root,
                &root.join(&marker),
                home.as_deref(),
            );
            let open = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.open(
                    Input::Project { root, marker },
                    &OpenCx::new(&settings, &lang)
                        .with_term(term)
                        .with_read_scope(scope.clone()),
                )
            }));
            let mut session = match open {
                Ok(Ok(session)) => session,
                result => {
                    let mut reply = ProjectReply::empty();
                    reply.error = Some(match result {
                        Ok(Err(error)) => error,
                        _ => Failure::external(Tr::new(panic_message)),
                    });
                    let _ = results.send(reply);
                    return;
                }
            };
            let enabled = |session: &dyn Session| {
                actions
                    .iter()
                    .map(|a| (a.clone(), session.enabled(a)))
                    .collect()
            };
            let mut initial = ProjectReply::empty();
            let cx = ViewCx::new(&settings, &lang)
                .with_term(term)
                .with_read_scope(scope.clone());
            refresh_chip(
                &mut initial,
                session.as_mut(),
                &cx,
                &fallback,
                &panic_message,
            );
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                enabled(session.as_ref())
            })) {
                Ok(value) => initial.enabled = value,
                Err(_) => {
                    initial.error = Some(Failure::external(Tr::new(panic_message.clone())));
                }
            }
            if results.send(initial).is_err() {
                return;
            }
            while let Ok(job) = jobs.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let cx = ViewCx::new(&settings, &lang)
                        .with_term(term)
                        .with_read_scope(scope.clone());
                    let mut reply = ProjectReply::empty();
                    match job {
                        ProjectJob::Chip => {
                            refresh_chip(
                                &mut reply,
                                session.as_mut(),
                                &cx,
                                &fallback,
                                &panic_message,
                            );
                        }
                        ProjectJob::View { key, req } => {
                            reply.views.push((
                                key,
                                Arc::new(content(
                                    session.view(&req, &cx)?,
                                    &req,
                                    &scope,
                                    &theme,
                                    &lang,
                                )?),
                            ));
                        }
                        ProjectJob::Act {
                            action,
                            context,
                            refresh,
                            chip,
                            mut effect,
                        } => {
                            let cx_act = ActCx::new(&settings, &lang)
                                .with_term(term)
                                .with_read_scope(scope.clone())
                                .with_project(context);
                            let outcome = super::plugin_host::act_session(
                                session.as_mut(),
                                &action,
                                &cx_act,
                            )?;
                            effect.outcome = Some(outcome);
                            reply.effect = Some(*effect);
                            for (key, req) in refresh {
                                match session
                                    .view(&req, &cx)
                                    .and_then(|view| content(view, &req, &scope, &theme, &lang))
                                {
                                    Ok(view) => reply.views.push((key, Arc::new(view))),
                                    Err(error) => {
                                        reply.error = Some(error);
                                    }
                                }
                            }
                            if chip {
                                refresh_chip(
                                    &mut reply,
                                    session.as_mut(),
                                    &cx,
                                    &fallback,
                                    &panic_message,
                                );
                            }
                        }
                    }
                    reply.enabled = enabled(session.as_ref());
                    Ok::<_, Failure>(reply)
                }));
                let reply = match result {
                    Ok(Ok(reply)) => reply,
                    error => {
                        let mut reply = ProjectReply::empty();
                        reply.error = Some(match error {
                            Ok(Err(error)) => error,
                            _ => Failure::external(Tr::new(panic_message.clone())),
                        });
                        reply
                    }
                };
                if results.send(reply).is_err() {
                    return;
                }
            }
        });
        Self {
            project,
            views: HashMap::new(),
            enabled: HashMap::new(),
            chip,
            chip_dirty: false,
            busy: true,
            acting: false,
            #[cfg(test)]
            view_request: None,
            dirty: Default::default(),
            ready: false,
            error: None,
            tx,
            rx,
            pending: None,
            invalidated: false,
        }
    }
}

fn refresh_chip(
    reply: &mut ProjectReply,
    session: &mut dyn Session,
    cx: &ViewCx,
    fallback: &Option<Chip>,
    panic_message: &str,
) {
    // A chip failure is reported without losing this job's views or action outcome.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.chip(cx)));
    reply.chip = Some(match result {
        Ok(Ok(Some(chip))) => Some(chip.clipped()),
        Ok(Ok(None)) => fallback.clone(),
        error => {
            reply.chip_error = Some(match error {
                Ok(Err(error)) => error,
                _ => Failure::external(Tr::new(panic_message.to_owned())),
            });
            fallback.clone()
        }
    });
}

/// The same text, Markdown and diff widgets the file previews use, prepared off the UI thread.
fn content(
    view: View,
    req: &ViewReq,
    scope: &ReadScope,
    theme: &str,
    lang: &str,
) -> Result<ProjectContent, Failure> {
    use crate::preview::markdown as md;
    use konoma_plugin::ratatui_draw::into_line;
    Ok(match view {
        View::List(list) => ProjectContent::List(list),
        View::Text(mut text) => {
            let lines = match text.source {
                TextSource::Lines(lines) => lines.into_iter().map(into_line).collect(),
                source => {
                    let raw = match source {
                        TextSource::Owned(text) => text,
                        TextSource::File(path) => {
                            if !scope.allows(&path) {
                                return Err(Failure::unsupported(Tr::new(crate::i18n::tr(
                                    crate::i18n::Lang::parse(lang),
                                    crate::i18n::Msg::ImageReasonOutsideReadDirs,
                                ))));
                            }
                            std::fs::read_to_string(path)?
                        }
                        TextSource::Lines(_) => unreachable!(),
                    };
                    match text.styler.as_mut() {
                        Some(styler) => styler
                            .style_block(0, &raw)
                            .into_iter()
                            .map(into_line)
                            .collect(),
                        None => raw
                            .lines()
                            .map(|l| ratatui::text::Line::raw(l.to_string()))
                            .collect(),
                    }
                }
            };
            ProjectContent::Lines(lines)
        }
        View::Diff(diff) => {
            let lines: Vec<_> = diff
                .hunks
                .into_iter()
                .flat_map(|h| h.lines)
                .map(|line| crate::git::DiffLine {
                    kind: match line.kind {
                        konoma_plugin::view::DiffKind::Added => crate::git::DiffLineKind::Added,
                        konoma_plugin::view::DiffKind::Removed => crate::git::DiffLineKind::Removed,
                        _ => crate::git::DiffLineKind::Context,
                    },
                    old_no: line.old_no.and_then(|n| n.try_into().ok()),
                    new_no: line.new_no.and_then(|n| n.try_into().ok()),
                    text: line.text,
                })
                .collect();
            ProjectContent::Diff(crate::preview::gitdiff::diff_lines(
                &lines,
                "",
                theme,
                req.cols as usize,
            ))
        }
        View::Document(doc) => {
            let (lines, _, _) = md::render_markdown_with_images_aligned(
                &doc.markdown,
                req.cols,
                md::CodeStyle::default(),
                theme,
                false,
                &doc.options.task_states,
                &|_, _| md::ImageSlot::Unavailable,
                &|_| md::MermaidSlot::Text,
                crate::i18n::tr(
                    crate::i18n::Lang::parse(lang),
                    crate::i18n::Msg::MermaidCaption,
                ),
                doc.options.alerts,
                &|_, _| md::MathSlot::Raw,
                false,
                md::BlockAligns {
                    table: md::BlockAlign::from_config(
                        &doc.options.table_align,
                        md::BlockAlign::Left,
                    ),
                    image: md::BlockAlign::from_config(
                        &doc.options.image_align,
                        md::BlockAlign::Center,
                    ),
                },
            );
            ProjectContent::Lines(lines)
        }
        View::Message(message) => {
            ProjectContent::Lines(message.lines.into_iter().map(into_line).collect())
        }
        _ => {
            return Err(Failure::unsupported(Tr::new(crate::i18n::tr(
                crate::i18n::Lang::parse(lang),
                crate::i18n::Msg::ProjectViewUnsupported,
            ))))
        }
    })
}

impl super::App {
    pub(super) fn project_request(&self, key: &ScreenKey) -> ViewReq {
        let mut project = ProjectView::new(&key.screen);
        if let Some(selection) = &key.selection {
            project = project.selection(selection);
        }
        ViewReq::new(
            if self.tab.preview_cols == 0 {
                80
            } else {
                self.tab.preview_cols
            },
            self.tab.preview_viewport.max(1),
        )
        .project(project)
    }

    pub(super) fn start_project_runtime(
        &mut self,
        project: crate::plugins::projects::MatchedProject,
    ) {
        let key = ProjectKey::from(&project);
        if self.project_runtimes.contains_key(&key) {
            return;
        }
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let Some(plugin) = registry.plugin(&project.plugin) else {
            return;
        };
        let runtime = ProjectRuntime::start(
            project.clone(),
            plugin,
            registry
                .manifest(&project.plugin)
                .map(|m| m.actions.iter().map(|a| a.id.clone()).collect())
                .unwrap_or_default(),
            crate::plugins::effective_settings(&self.cfg, &project.plugin),
            self.plugin_lang().into(),
            self.cfg.ui.theme.code_theme.clone(),
            TermCaps::new().colors(self.plugin_colors),
            self.cfg.plugins.get(&project.plugin).cloned(),
            crate::plugins::read_scope::is_host_owned(&registry, &project.plugin),
            crate::plugins::read_scope::home_dir(),
            crate::i18n::tr(self.lang, crate::i18n::Msg::ProjectWorkerFailed).into(),
        );
        self.project_runtimes.insert(key, runtime);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::proc::{
        client::{Config, ProcPlugin, Spawned},
        frame,
        proto::{Answer, FromChild, Hello, HelloReply, Reply, ToChild, WState},
        wire::{layout_to_wire, manifest_to_wire},
    };
    use konoma_plugin::{Manifest, ProjectDecl};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    #[test]
    fn project_chip_old_child_uses_localized_declaration_without_unknown_call() {
        let dir = crate::test_support::unique_tmp("konoma_project_chip_old_child");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("side")).unwrap();
        let declaration = ProjectDecl::new().chip(Tr::new("declared chip").ja("宣言チップ"));
        let manifest = Manifest::new("old-project", "1").project(declaration.clone());
        let (host, mut child) = UnixStream::pair().unwrap();
        let peer = std::thread::spawn(move || {
            let _: Hello = frame::recv(&mut child).unwrap().unwrap();
            let mut manifest = manifest_to_wire(&manifest).unwrap();
            manifest.session_chip = false;
            frame::send(
                &mut child,
                &HelloReply::Ack {
                    version: 2,
                    manifest,
                },
            )
            .unwrap();
            let ToChild::Open { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
                panic!("expected project open");
            };
            frame::send(
                &mut child,
                &FromChild {
                    rid,
                    answer: Answer::Ok(Reply::Opened {
                        state: WState {
                            layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                            enabled: Vec::new(),
                        },
                    }),
                },
            )
            .unwrap();
            loop {
                match frame::recv::<_, ToChild>(&mut child).unwrap() {
                    Some(ToChild::Close { .. }) => {}
                    Some(ToChild::Shutdown) | None => break,
                    other => panic!("old child received unknown call: {other:?}"),
                }
            }
        });
        let connection = Arc::new(std::sync::Mutex::new(Some(host)));
        let plugin = Arc::new(
            ProcPlugin::start(
                move || {
                    let host = connection.lock().unwrap().take().expect("one launch");
                    let kill = host.try_clone()?;
                    Ok(Spawned {
                        reader: Box::new(host.try_clone()?),
                        writer: Box::new(host),
                        stderr: None,
                        pid: None,
                        kill: Box::new(move || {
                            let _ = kill.shutdown(std::net::Shutdown::Both);
                        }),
                    })
                },
                Config {
                    scratch_root: dir.join("side"),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let runtime = ProjectRuntime::start(
            crate::plugins::projects::MatchedProject {
                plugin: "old-project".into(),
                root: dir.to_path_buf(),
                marker: "project".into(),
                declaration,
            },
            plugin.clone(),
            Vec::new(),
            Settings::empty().clone(),
            "ja".into(),
            String::new(),
            TermCaps::new(),
            None,
            false,
            None,
            "worker failed".into(),
        );
        let reply = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(reply.error.is_none());
        assert!(reply.chip_error.is_none());
        assert_eq!(reply.chip.unwrap().unwrap().text.plain(), "宣言チップ");
        drop(runtime);
        plugin.stop();
        peer.join().unwrap();
    }
}
