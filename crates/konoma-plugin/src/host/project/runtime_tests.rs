use crate::host::project::{
    MatchedProject, ProjectContent, ProjectEnv, ProjectJob, ProjectMessages, ProjectReply,
    ProjectRuntime, ScreenKey,
};
use crate::view::{View, ViewReq};
use crate::{ActCx, Failure, Input, Manifest, OpenCx, Outcome, Plugin, Session, Tr, ViewCx};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

fn messages() -> ProjectMessages {
    ProjectMessages {
        worker_failed: "worker failed".into(),
        no_changes: "No changes".into(),
        mermaid_caption: "Mermaid".into(),
        view_unsupported: "unsupported".into(),
        outside_read_dirs: "outside read dirs".into(),
        invalid_watch_paths: Tr::new("Invalid project watch paths"),
    }
}

fn env() -> ProjectEnv {
    ProjectEnv {
        settings: crate::Settings::empty().clone(),
        lang: "en".into(),
        theme: "TwoDark".into(),
        term: crate::TermCaps::new(),
        read_scope: crate::ReadScope::unrestricted(),
        messages: messages(),
    }
}

struct TestPlugin(Mutex<Option<Box<dyn Session>>>);

impl Plugin for TestPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("owner-test", "1")
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(self.0.lock().unwrap().take().unwrap())
    }
}

fn runtime(session: impl Session + 'static) -> ProjectRuntime<Option<Outcome>> {
    let project = MatchedProject {
        plugin: "owner-test".into(),
        root: "/project-owner-test".into(),
        marker: "marker".into(),
        declaration: crate::ProjectDecl::new(),
        session_watch_paths: None,
    };
    let runtime = ProjectRuntime::start(
        project,
        Arc::new(TestPlugin(Mutex::new(Some(Box::new(session))))),
        Vec::new(),
        None,
        env,
        |effect, outcome| *effect = Some(outcome),
    );
    let initial = reply(&runtime);
    assert!(initial.error.is_none());
    runtime
}

fn reply(runtime: &ProjectRuntime<Option<Outcome>>) -> ProjectReply<Option<Outcome>> {
    runtime.rx.recv_timeout(TIMEOUT).expect("owner reply")
}

fn key(tab: u64) -> ScreenKey {
    ScreenKey::new("text", tab)
}

fn act(action: &str, refresh: bool, watch: bool) -> ProjectJob<Option<Outcome>> {
    ProjectJob::Act {
        action: action.into(),
        context: crate::ProjectAct::new().screen("text"),
        refresh: if refresh {
            vec![(key(1), ViewReq::new(80, 20))]
        } else {
            Vec::new()
        },
        chip: false,
        watch,
        effect: Box::new(None),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Enter(String, ThreadId),
    Exit(String, ThreadId),
    Closed(u64),
}

struct SerialSession {
    events: mpsc::Sender<Event>,
    release: mpsc::Receiver<()>,
}

impl Session for SerialSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        unreachable!()
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        let thread = std::thread::current().id();
        self.events
            .send(Event::Enter(action.into(), thread))
            .unwrap();
        self.release.recv_timeout(TIMEOUT).unwrap();
        self.events
            .send(Event::Exit(action.into(), thread))
            .unwrap();
        Ok(Outcome::Redraw)
    }

    fn tab_closed(&mut self, tab: u64) {
        self.events.send(Event::Closed(tab)).unwrap();
    }
}

#[test]
fn project_actions_run_serially_on_one_owner() {
    let (events, observed) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let runtime = runtime(SerialSession {
        events,
        release: held,
    });
    runtime.tx.send(act("first", false, false)).unwrap();
    let Event::Enter(action, owner) = observed.recv_timeout(TIMEOUT).unwrap() else {
        panic!("first action must enter")
    };
    assert_eq!(action, "first");
    assert_ne!(owner, std::thread::current().id());
    runtime.tx.send(act("second", false, false)).unwrap();
    assert!(matches!(
        runtime.tx.try_send(ProjectJob::Chip),
        Err(mpsc::TrySendError::Full(_))
    ));
    assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).unwrap();
    assert_eq!(
        observed.recv_timeout(TIMEOUT).unwrap(),
        Event::Exit("first".into(), owner)
    );
    assert!(!reply(&runtime).act_failed);
    assert_eq!(
        observed.recv_timeout(TIMEOUT).unwrap(),
        Event::Enter("second".into(), owner)
    );
    release.send(()).unwrap();
    assert_eq!(
        observed.recv_timeout(TIMEOUT).unwrap(),
        Event::Exit("second".into(), owner)
    );
    assert!(!reply(&runtime).act_failed);
}

struct FailureSession {
    acts: Arc<Mutex<usize>>,
    watch_calls: usize,
    view_error: bool,
}

impl Session for FailureSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        if self.view_error {
            return Err(Failure::external(Tr::new("view error")));
        }
        panic!("view panic")
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        *self.acts.lock().unwrap() += 1;
        match action {
            "panic" => panic!("act panic"),
            "error" => Err(Failure::external(Tr::new("act error"))),
            _ => Ok(Outcome::Redraw),
        }
    }

    fn watch_paths(&mut self, _: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        self.watch_calls += 1;
        if self.watch_calls > 1 {
            Err(Failure::external(Tr::new("watch error")))
        } else {
            Ok(None)
        }
    }
}

#[test]
fn project_act_panic_sets_act_failed_but_view_panic_does_not() {
    let runtime = runtime(FailureSession {
        acts: Arc::default(),
        watch_calls: 0,
        view_error: false,
    });
    for action in ["panic", "error"] {
        runtime.tx.send(act(action, false, false)).unwrap();
        let result = reply(&runtime);
        assert!(result.act_failed);
        assert!(result.error.is_some());
        assert_eq!(
            result.error.unwrap().message.get("en"),
            if action == "panic" {
                "worker failed"
            } else {
                "act error"
            }
        );
        assert!(result.effect.is_none());
    }
    runtime
        .tx
        .send(ProjectJob::View {
            key: key(1),
            req: ViewReq::new(80, 20),
        })
        .unwrap();
    let result = reply(&runtime);
    assert!(!result.act_failed);
    assert_eq!(result.error.unwrap().message.get("en"), "worker failed");
}

#[test]
fn project_successful_act_survives_follow_up_failures_without_resubmission() {
    for (refresh, watch, view_error) in [
        (true, false, false),
        (true, false, true),
        (false, true, false),
        (true, true, false),
    ] {
        let acts = Arc::new(Mutex::new(0));
        let runtime = runtime(FailureSession {
            acts: acts.clone(),
            watch_calls: 0,
            view_error,
        });
        runtime.tx.send(act("success", refresh, watch)).unwrap();
        let result = reply(&runtime);
        assert!(!result.act_failed);
        assert!(result.error.is_some());
        assert!(
            result.effect.is_some(),
            "successful action outcome must survive"
        );
        assert_eq!(result.effect.unwrap(), Some(Outcome::Redraw));
        runtime.tx.send(ProjectJob::Chip).unwrap();
        assert!(!reply(&runtime).act_failed);
        assert_eq!(
            *acts.lock().unwrap(),
            1,
            "follow-up must never repeat the action"
        );
    }
}

#[test]
fn project_tabs_closed_drops_cached_views_and_notifies_owner() {
    let (events, observed) = mpsc::channel();
    let (_release, held) = mpsc::channel();
    let mut runtime = runtime(SerialSession {
        events,
        release: held,
    });
    for tab in [1, 2] {
        runtime
            .views
            .insert(key(tab), Arc::new(ProjectContent::Lines(Vec::new())));
        runtime
            .contexts
            .insert(key(tab), crate::ProjectView::new("text").tab(tab));
        runtime.dirty.insert(key(tab));
    }
    runtime.tab_closed(1);
    assert!(!runtime.views.contains_key(&key(1)));
    assert!(!runtime.contexts.contains_key(&key(1)));
    assert!(!runtime.dirty.contains(&key(1)));
    assert!(runtime.views.contains_key(&key(2)));
    assert!(runtime.contexts.contains_key(&key(2)));
    assert!(runtime.dirty.contains(&key(2)));
    assert_eq!(runtime.closed_tabs.iter().copied().collect::<Vec<_>>(), [1]);
    runtime
        .tx
        .send(ProjectJob::TabsClosed { tabs: vec![1] })
        .unwrap();
    assert!(reply(&runtime).error.is_none());
    assert_eq!(observed.recv_timeout(TIMEOUT).unwrap(), Event::Closed(1));
}
