//! One owner thread per project. Neither session calls nor file/document rendering run on the UI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use crate::project::{Chip, ProjectView};
use crate::view::ViewReq;
use crate::{
    ActCx, Failure, Input, OpenCx, Plugin, ReadScope, Session, Settings, TermCaps, Tr, ViewCx,
};

use super::{content::content, MatchedProject, ProjectContent};

/// Host decisions, prepared on the owner before opening the session. No config or policy is
/// interpreted by the SDK. The factory passed to `start` can resolve the read scope off the UI.
pub struct ProjectEnv {
    pub settings: Settings,
    pub lang: String,
    pub theme: String,
    pub term: TermCaps,
    pub read_scope: ReadScope,
    pub messages: ProjectMessages,
}

/// Already translated by the host; the SDK has no message catalogue.
pub struct ProjectMessages {
    pub worker_failed: String,
    pub no_changes: String,
    pub mermaid_caption: String,
    pub view_unsupported: String,
    pub outside_read_dirs: String,
    pub invalid_watch_paths: Tr,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScreenKey {
    pub tab: u64,
    pub screen: String,
    pub selection: Option<String>,
}

impl ScreenKey {
    pub fn new(screen: impl Into<String>, tab: u64) -> Self {
        Self {
            tab,
            screen: screen.into(),
            selection: None,
        }
    }
}

pub enum ProjectJob<E> {
    TabsClosed {
        tabs: Vec<u64>,
    },
    Watch,
    Chip,
    View {
        key: ScreenKey,
        req: ViewReq,
    },
    Act {
        action: String,
        context: crate::project::ProjectAct,
        refresh: Vec<(ScreenKey, ViewReq)>,
        chip: bool,
        watch: bool,
        effect: Box<E>,
    },
}

pub struct ProjectReply<E> {
    pub watch_paths: Option<Vec<PathBuf>>,
    pub views: Vec<(ScreenKey, Arc<ProjectContent>)>,
    pub enabled: HashMap<String, bool>,
    pub chip: Option<Option<Chip>>,
    pub chip_error: Option<Failure>,
    pub effect: Option<E>,
    pub error: Option<Failure>,
    /// Only an error or panic from `act` sets this; follow-up failures never do.
    pub act_failed: bool,
    pub contexts: HashMap<ScreenKey, ProjectView>,
}

impl<E> ProjectReply<E> {
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

pub struct ProjectRuntime<E: Send + 'static, I = ()> {
    pub closed_tabs: std::collections::BTreeSet<u64>,
    pub watch_paths: Option<Vec<PathBuf>>,
    pub refresh_watch: bool,
    pub project: MatchedProject,
    pub views: HashMap<ScreenKey, Arc<ProjectContent>>,
    pub contexts: HashMap<ScreenKey, ProjectView>,
    pub enabled: HashMap<String, bool>,
    pub chip: Option<Chip>,
    pub chip_dirty: bool,
    pub busy: bool,
    pub acting: bool,
    pub pending_retry: Option<Box<I>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub view_request: Option<ScreenKey>,
    pub dirty: std::collections::HashSet<ScreenKey>,
    pub ready: bool,
    pub error: Option<Failure>,
    pub tx: mpsc::SyncSender<ProjectJob<E>>,
    pub rx: mpsc::Receiver<ProjectReply<E>>,
    pub pending: Option<ProjectReply<E>>,
    pub invalidated: bool,
}

impl<E: Send + 'static, I> ProjectRuntime<E, I> {
    /// Start one serial session owner. Both channels retain their single waiting slot.
    /// `chip` is the localized declaration fallback, available before the first reply.
    /// `complete_effect` attaches the outcome to opaque host data, on the owner thread.
    pub fn start(
        project: MatchedProject,
        plugin: Arc<dyn Plugin>,
        actions: Vec<String>,
        chip: Option<Chip>,
        env: impl FnOnce() -> ProjectEnv + Send + 'static,
        complete_effect: fn(&mut E, crate::Outcome),
    ) -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        let root = project.root.clone();
        let marker = project.marker.clone();
        let declaration = project.declaration.clone();
        let fallback = chip.clone();
        std::thread::spawn(move || {
            let ProjectEnv {
                settings,
                lang,
                theme,
                term,
                read_scope: scope,
                messages,
            } = env();
            let panic_message = &messages.worker_failed;
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
                        _ => Failure::external(Tr::new(panic_message.clone())),
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
                Ok(Ok(paths)) => {
                    match resolved_watch_paths(paths, &root, &declaration, &messages) {
                        Ok(paths) => initial.watch_paths = Some(paths),
                        Err(error) => initial.error = Some(error),
                    }
                }
                Ok(Err(error)) => initial.error = Some(error),
                Err(_) => initial.error = Some(Failure::external(Tr::new(panic_message.clone()))),
            }
            refresh_chip(
                &mut initial,
                session.as_mut(),
                &cx,
                &fallback,
                panic_message,
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
                let mut reply = ProjectReply::empty();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let cx = ViewCx::new(&settings, &lang)
                        .with_term(term)
                        .with_read_scope(scope.clone());
                    match job {
                        ProjectJob::TabsClosed { tabs } => {
                            for tab in tabs {
                                session.tab_closed(tab);
                            }
                        }
                        ProjectJob::Watch => {
                            reply.watch_paths = Some(resolved_watch_paths(
                                session.watch_paths(&cx)?,
                                &root,
                                &declaration,
                                &messages,
                            )?);
                            refresh_chip(
                                &mut reply,
                                session.as_mut(),
                                &cx,
                                &fallback,
                                panic_message,
                            );
                        }
                        ProjectJob::Chip => {
                            refresh_chip(
                                &mut reply,
                                session.as_mut(),
                                &cx,
                                &fallback,
                                panic_message,
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
                                    &messages,
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
                                    plugin_host::act_session(session.as_mut(), &action, &cx_act)
                                }));
                            let outcome = match result {
                                Ok(Ok(outcome)) => outcome,
                                error => {
                                    reply.act_failed = true;
                                    reply.error = Some(match error {
                                        Ok(Err(error)) => error,
                                        _ => Failure::external(Tr::new(panic_message.clone())),
                                    });
                                    return Ok(());
                                }
                            };
                            complete_effect(&mut effect, outcome);
                            reply.effect = Some(*effect);
                            if watch {
                                reply.watch_paths = Some(resolved_watch_paths(
                                    session.watch_paths(&cx)?,
                                    &root,
                                    &declaration,
                                    &messages,
                                )?);
                            }
                            for (key, req) in refresh {
                                if let Some(context) = &req.project {
                                    reply.contexts.insert(key.clone(), context.clone());
                                }
                                match session
                                    .view(&req, &cx)
                                    .and_then(|view| content(view, &req, &scope, &theme, &messages))
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
                                    panic_message,
                                );
                            }
                        }
                    }
                    reply.enabled = enabled(session.as_ref());
                    Ok::<_, Failure>(())
                }));
                match result {
                    Ok(Ok(())) => {}
                    error => {
                        // A failed batch keeps the previous views, as before, but an action
                        // already completed must still deliver its outcome to the host.
                        let effect = reply.effect.take();
                        reply = ProjectReply::empty();
                        reply.effect = effect;
                        reply.error = Some(match error {
                            Ok(Err(error)) => error,
                            _ => Failure::external(Tr::new(panic_message.clone())),
                        });
                    }
                }
                if results.send(reply).is_err() {
                    return;
                }
            }
        });
        Self {
            watch_paths: None,
            closed_tabs: Default::default(),
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
            #[cfg(any(test, feature = "test-hooks"))]
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

    /// Forget a closed tab immediately; its session notification follows earlier jobs.
    /// The host also discards any pending invocation associated with this tab.
    pub fn tab_closed(&mut self, tab: u64) {
        self.views.retain(|key, _| key.tab != tab);
        self.contexts.retain(|key, _| key.tab != tab);
        self.dirty.retain(|key| key.tab != tab);
        self.closed_tabs.insert(tab);
    }
}

fn resolved_watch_paths(
    paths: Option<Vec<PathBuf>>,
    root: &std::path::Path,
    declaration: &crate::ProjectDecl,
    messages: &ProjectMessages,
) -> Result<Vec<PathBuf>, Failure> {
    let mut paths = paths.unwrap_or_else(|| {
        declaration
            .watch_paths
            .iter()
            .map(|p| root.join(p))
            .collect()
    });
    if paths.len() > 512 || paths.iter().any(|p| !p.is_absolute()) {
        return Err(Failure::unsupported(messages.invalid_watch_paths.clone()));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn refresh_chip<E>(
    reply: &mut ProjectReply<E>,
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

// The executor is deliberately policy-free: the host guards the single job constructor.
mod plugin_host {
    pub(super) fn act_session(
        session: &mut dyn crate::Session,
        action: &str,
        cx: &crate::ActCx<'_>,
    ) -> Result<crate::Outcome, crate::Failure> {
        session.act(action, cx)
    }
}
