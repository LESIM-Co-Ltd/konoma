//! One owner thread per project. Neither session calls nor file/document rendering run on the UI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use konoma_plugin::project::{Chip, ProjectView};
use konoma_plugin::view::{TextSource, View, ViewReq};
use konoma_plugin::{
    ActCx, Failure, Input, OpenCx, Plugin, ReadScope, Session, Settings, TermCaps, Tr, ViewCx,
};

use super::project_screens::{
    ProjectContent, ProjectEffect, ProjectInvocation, ProjectKey, ScreenKey,
};
use super::PathStyle;

pub(super) enum ProjectJob {
    Watch,
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
        watch: bool,
        effect: Box<ProjectEffect>,
    },
}

pub(super) struct ProjectReply {
    pub watch_paths: Option<Vec<PathBuf>>,
    pub views: Vec<(ScreenKey, Arc<ProjectContent>)>,
    pub enabled: HashMap<String, bool>,
    pub chip: Option<Option<Chip>>,
    pub chip_error: Option<Failure>,
    pub effect: Option<ProjectEffect>,
    pub error: Option<Failure>,
    /// Only an error or panic from `act` sets this; follow-up failures never do.
    pub act_failed: bool,
    pub contexts: HashMap<ScreenKey, ProjectView>,
}

impl ProjectReply {
    fn empty() -> Self {
        Self {
            watch_paths: None,
            views: Vec::new(),
            enabled: HashMap::new(),
            chip: None,
            chip_error: None,
            effect: None,
            error: None,
            act_failed: false,
            contexts: HashMap::new(),
        }
    }
}

pub(crate) struct ProjectRuntime {
    pub(super) watch_paths: Option<Vec<PathBuf>>,
    pub(super) refresh_watch: bool,
    pub(crate) project: crate::plugins::projects::MatchedProject,
    pub(super) views: HashMap<ScreenKey, Arc<ProjectContent>>,
    pub(super) contexts: HashMap<ScreenKey, ProjectView>,
    pub(super) enabled: HashMap<String, bool>,
    pub(super) chip: Option<Chip>,
    pub(super) chip_dirty: bool,
    pub(super) busy: bool,
    pub(super) acting: bool,
    pub(super) pending_retry: Option<Box<ProjectInvocation>>,
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
        let declaration = project.declaration.clone();
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
                    Input::Project {
                        root: root.clone(),
                        marker,
                    },
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
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                session.watch_paths(&cx)
            })) {
                Ok(Ok(paths)) => match resolved_watch_paths(paths, &root, &declaration) {
                    Ok(paths) => initial.watch_paths = Some(paths),
                    Err(error) => initial.error = Some(error),
                },
                Ok(Err(error)) => initial.error = Some(error),
                Err(_) => initial.error = Some(Failure::external(Tr::new(panic_message.clone()))),
            }
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
                        ProjectJob::Watch => {
                            reply.watch_paths = Some(resolved_watch_paths(
                                session.watch_paths(&cx)?,
                                &root,
                                &declaration,
                            )?);
                            refresh_chip(
                                &mut reply,
                                session.as_mut(),
                                &cx,
                                &fallback,
                                &panic_message,
                            );
                        }
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
                            if let Some(context) = &req.project {
                                reply.contexts.insert(key.clone(), context.clone());
                            }
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
                            watch,
                            mut effect,
                        } => {
                            let cx_act = ActCx::new(&settings, &lang)
                                .with_term(term)
                                .with_read_scope(scope.clone())
                                .with_project(context);
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    super::plugin_host::act_session(
                                        session.as_mut(),
                                        &action,
                                        &cx_act,
                                    )
                                }));
                            let outcome = match result {
                                Ok(Ok(outcome)) => outcome,
                                error => {
                                    reply.act_failed = true;
                                    reply.error = Some(match error {
                                        Ok(Err(error)) => error,
                                        _ => Failure::external(Tr::new(panic_message.clone())),
                                    });
                                    return Ok(reply);
                                }
                            };
                            effect.outcome = Some(outcome);
                            reply.effect = Some(*effect);
                            if watch {
                                reply.watch_paths = Some(resolved_watch_paths(
                                    session.watch_paths(&cx)?,
                                    &root,
                                    &declaration,
                                )?);
                            }
                            for (key, req) in refresh {
                                if let Some(context) = &req.project {
                                    reply.contexts.insert(key.clone(), context.clone());
                                }
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
            watch_paths: None,
            refresh_watch: false,
            project,
            views: HashMap::new(),
            contexts: HashMap::new(),
            enabled: HashMap::new(),
            chip,
            chip_dirty: false,
            busy: true,
            acting: false,
            pending_retry: None,
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

fn resolved_watch_paths(
    paths: Option<Vec<PathBuf>>,
    root: &std::path::Path,
    declaration: &konoma_plugin::ProjectDecl,
) -> Result<Vec<PathBuf>, Failure> {
    let mut paths = paths.unwrap_or_else(|| {
        declaration
            .watch_paths
            .iter()
            .map(|p| root.join(p))
            .collect()
    });
    if paths.len() > 512 || paths.iter().any(|p| !p.is_absolute()) {
        return Err(Failure::unsupported(
            konoma_plugin::t!(en: "Invalid project watch paths", ja: "プロジェクトの監視パスが不正です"),
        ));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
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
            let mut previous = None;
            let lines: Vec<_> = diff
                .hunks
                .into_iter()
                .flat_map(|h| {
                    let marker = if h.path != previous {
                        h.path.as_ref().map(|path| konoma_plugin::view::DiffLine {
                            kind: konoma_plugin::view::DiffKind::Context,
                            old_no: None,
                            new_no: None,
                            text: path.clone(),
                        })
                    } else {
                        None
                    };
                    previous = h.path;
                    marker.into_iter().chain(h.lines)
                })
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
            ProjectContent::Diff(super::project_screens::ProjectDiff::new(
                lines,
                diff.header.into_iter().map(into_line).collect(),
                theme.into(),
                crate::i18n::tr(
                    crate::i18n::Lang::parse(lang),
                    crate::i18n::Msg::GitNoChanges,
                )
                .into(),
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
        let tab = if self.tab.id == key.tab {
            &self.tab
        } else {
            self.tabs
                .iter()
                .find(|tab| tab.id == key.tab)
                .unwrap_or(&self.tab)
        };
        let mut project = ProjectView::new(&key.screen)
            .tab(key.tab)
            .icons(self.cfg.ui.icons)
            .paths(&tab.root, &tab.open_dir)
            .home_dir(crate::plugins::read_scope::home_dir());
        if let Some(selection) = &key.selection {
            project = project.selection(selection);
        }
        ViewReq::new(
            if tab.preview_cols == 0 {
                80
            } else {
                tab.preview_cols
            },
            tab.preview_viewport.max(1),
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
        let mut settings = crate::plugins::effective_settings(&self.cfg, &project.plugin);
        if settings.get("path_style").is_none() {
            settings.set(
                "path_style",
                konoma_plugin::Value::Str(
                    match self.path_style {
                        PathStyle::Relative => "relative",
                        PathStyle::Home => "home",
                        PathStyle::Full => "full",
                    }
                    .into(),
                ),
            );
        }
        let runtime = ProjectRuntime::start(
            project.clone(),
            plugin,
            registry
                .manifest(&project.plugin)
                .map(|m| m.actions.iter().map(|a| a.id.clone()).collect())
                .unwrap_or_default(),
            settings,
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
#[path = "project_worker_tests.rs"]
mod tests;
