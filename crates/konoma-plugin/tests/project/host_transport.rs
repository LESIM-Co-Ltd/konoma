//! Exercise project data through the real client and server, including old peers.

use super::*;
use konoma_plugin::proc::{
    client::{Config, ProcPlugin, Spawned},
    frame,
    proto::*,
    serve_io,
    wire::*,
    ServeOptions,
};
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use std::thread::JoinHandle;

pub(crate) type Threads = Arc<Mutex<Vec<JoinHandle<io::Result<()>>>>>;

pub(crate) fn connect(
    run: impl Fn(UnixStream) -> io::Result<()> + Send + Sync + 'static,
) -> (ProcPlugin, Threads, konoma_test_support::TmpDir) {
    let dir = konoma_test_support::unique_tmp("project_host_api");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let threads = Threads::default();
    let started = threads.clone();
    let run = Arc::new(run);
    let plugin = ProcPlugin::start(
        move || {
            let (host, child) = UnixStream::pair()?;
            let kill = host.try_clone()?;
            let run = run.clone();
            started
                .lock()
                .unwrap()
                .push(std::thread::spawn(move || run(child)));
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
            scratch_root: dir.as_path().to_path_buf(),
            ..Config::default()
        },
    )
    .unwrap();
    (plugin, threads, dir)
}

pub(crate) fn finish(plugin: ProcPlugin, threads: Threads) {
    plugin.stop();
    drop(plugin);
    for thread in threads.lock().unwrap().drain(..) {
        thread.join().unwrap().unwrap();
    }
}

#[derive(Clone)]
struct ApiPlugin {
    project: bool,
    calls: Arc<AtomicUsize>,
}
impl Plugin for ApiPlugin {
    fn manifest(&self) -> Manifest {
        if self.project {
            let mut action = Act::new("write").writes();
            action.refresh = targets();
            manifest()
                .read_only()
                .action(Act::new("push"))
                .action(action)
        } else {
            Manifest::new("p", "1").action(Act::new("write"))
        }
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if self.project {
            assert!(
                matches!(input, Input::Project { root, marker } if root == std::path::Path::new("/p") && marker == ".git")
            );
        }
        Ok(Box::new(self.clone()))
    }
}
impl Session for ApiPlugin {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.project {
            assert_eq!(
                req.project,
                Some(ProjectView::new("branches").selection("branch-2"))
            );
            Ok(View::List(ListView {
                rows: vec![ListRow::new("branch", vec![]).value("branch-2")],
            }))
        } else {
            Ok(View::List(ListView::default()))
        }
    }
    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if !self.project {
            return Ok(Outcome::Redraw);
        }
        assert_eq!(cx.project, Some(answers()));
        Ok(if action == "push" {
            Outcome::PushScreen {
                screen: "branches".into(),
                selection: Some("branch-2".into()),
            }
        } else {
            Outcome::Flash("written".into())
        })
    }
    fn key(&mut self, _: &Key, cx: &ActCx) -> Result<Outcome, Failure> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if !self.project {
            return Ok(Outcome::Redraw);
        }
        assert_eq!(cx.project, Some(answers()));
        Ok(Outcome::Flash("key".into()))
    }
}

#[test]
fn project_host_api_crosses_the_client_and_server() {
    let calls = Arc::new(AtomicUsize::new(0));
    let api = ApiPlugin {
        project: true,
        calls: calls.clone(),
    };
    let (plugin, threads, _dir) = connect(move |child| {
        serve_io(
            api.clone(),
            child.try_clone()?,
            child,
            &ServeOptions::default(),
        )
    });
    assert!(plugin.manifest().read_only);
    assert_eq!(plugin.manifest().actions[1].refresh, targets());
    let mut session = plugin
        .open(
            Input::Project {
                root: "/p".into(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let cx = ActCx::bare().with_project(answers());
    let Outcome::PushScreen { screen, selection } = session.act("push", &cx).unwrap() else {
        panic!("push")
    };
    let req = ViewReq::new(80, 24).project(ProjectView::new(screen).selection(selection.unwrap()));
    let View::List(list) = session.view(&req, &ViewCx::bare()).unwrap() else {
        panic!("list")
    };
    assert_eq!(
        ProjectRow::from(&list.rows[0]),
        ProjectRow::new("branch", "branch-2")
    );
    assert_eq!(
        session.act("write", &cx).unwrap(),
        Outcome::Flash("written".into())
    );
    assert_eq!(
        session.key(&Key::char('x'), &cx).unwrap(),
        Outcome::Flash("key".into())
    );
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    drop(session);
    finish(plugin, threads);
}

#[test]
fn protocol_one_client_never_sends_project_calls() {
    let (plugin, threads, _dir) = connect(|mut child| {
        let _: Hello = frame::recv(&mut child).unwrap().unwrap();
        frame::send(
            &mut child,
            &HelloReply::Ack {
                version: 1,
                manifest: manifest_to_wire(&Manifest::new("p", "1")).unwrap(),
            },
        )
        .unwrap();
        let ToChild::Open { sid: _, rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
            panic!("open")
        };
        frame::send(
            &mut child,
            &FromChild {
                rid,
                answer: Answer::Ok(Reply::Opened {
                    state: WState {
                        layout: layout_to_wire(&konoma_plugin::view::Layout::default()),
                        enabled: vec![],
                    },
                }),
            },
        )
        .unwrap();
        // No view, act or key containing project data may reach a version-1 peer.
        while let Some(msg) = frame::recv::<_, ToChild>(&mut child).unwrap() {
            match msg {
                ToChild::Close { .. } => {}
                ToChild::Shutdown => break,
                other => panic!("unexpected: {other:?}"),
            }
        }
        Ok(())
    });
    let mut session = plugin
        .open(
            Input::File {
                path: "/unused".into(),
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let req = ViewReq::new(80, 24).project(ProjectView::new("branches"));
    let cx = ActCx::bare().with_project(answers());
    let failures = [
        session.view(&req, &ViewCx::bare()).unwrap_err(),
        session.act("write", &cx).unwrap_err(),
        session.key(&Key::char('x'), &cx).unwrap_err(),
    ];
    for f in failures {
        assert_eq!(f.kind, FailureKind::Corrupt);
        assert_eq!(
            f.detail.as_deref(),
            Some("project data requires protocol 2, negotiated 1")
        );
    }
    drop(session);
    finish(plugin, threads);
}

#[test]
fn protocol_one_server_rejects_project_calls_before_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let api = ApiPlugin {
        project: false,
        calls: calls.clone(),
    };
    let (mut host, child) = UnixStream::pair().unwrap();
    let thread = std::thread::spawn(move || {
        serve_io(
            api,
            child.try_clone().unwrap(),
            child,
            &ServeOptions::default(),
        )
    });
    frame::send(
        &mut host,
        &Hello {
            versions: vec![1],
            host_version: "test".into(),
            side_dir: path_to_wire(std::path::Path::new("/unused")),
            lang: "en".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        frame::recv::<_, HelloReply>(&mut host).unwrap().unwrap(),
        HelloReply::Ack { version: 1, .. }
    ));
    frame::send(
        &mut host,
        &ToChild::Open {
            sid: 1,
            rid: 1,
            input: WInput::File {
                path: path_to_wire(std::path::Path::new("/unused")),
                hint: None,
            },
            cx: WCx::new("en"),
        },
    )
    .unwrap();
    assert!(matches!(
        frame::recv::<_, FromChild>(&mut host)
            .unwrap()
            .unwrap()
            .answer,
        Answer::Ok(Reply::Opened { .. })
    ));
    let cx = WCx::new("en").project(WProjectAct::new());
    for msg in [
        ToChild::View {
            sid: 1,
            rid: 2,
            req: WViewReq::new(80, 24).project(WProjectView::new("branches")),
            cx: WCx::new("en"),
        },
        ToChild::Act {
            sid: 1,
            rid: 3,
            action: "write".into(),
            cx: cx.clone(),
        },
        ToChild::Key {
            sid: 1,
            rid: 4,
            key: key_to_wire(&Key::char('x')),
            cx,
        },
    ] {
        frame::send(&mut host, &msg).unwrap();
        let answer: FromChild = frame::recv(&mut host).unwrap().unwrap();
        let Answer::Err(f) = answer.answer else {
            panic!("project data was dispatched")
        };
        assert_eq!(failure_from_wire(f).kind, FailureKind::Corrupt);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    frame::send(&mut host, &ToChild::Shutdown).unwrap();
    thread.join().unwrap().unwrap();
}
