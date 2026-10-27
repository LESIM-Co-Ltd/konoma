//! The child side driven by hand: a raw host over socket pairs against `serve_io`.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use konoma_plugin::proc::frame;
use konoma_plugin::proc::proto::*;
use konoma_plugin::proc::wire::*;
use konoma_plugin::proc::{serve_io, ServeOptions};
use konoma_plugin::*;
use konoma_test_support::unique_tmp;

use crate::common::*;

struct Host {
    to: UnixStream,
    from: UnixStream,
    child: Option<JoinHandle<std::io::Result<()>>>,
    ctl: Arc<Ctl>,
    rid: u64,
}

impl Host {
    fn start(opts: ServeOptions) -> Host {
        let (to, child_r) = UnixStream::pair().unwrap();
        let (child_w, from) = UnixStream::pair().unwrap();
        from.set_read_timeout(Some(PATIENCE)).unwrap();
        let ctl = Arc::new(Ctl::default());
        let plugin = TestPlugin {
            ctl: Arc::clone(&ctl),
        };
        let child = std::thread::spawn(move || serve_io(plugin, child_r, child_w, &opts));
        Host {
            to,
            from,
            child: Some(child),
            ctl,
            rid: 0,
        }
    }

    fn hello(&mut self, versions: Vec<u32>, side_dir: &std::path::Path) -> HelloReply {
        let h = Hello {
            versions,
            host_version: "t".into(),
            side_dir: path_to_wire(side_dir),
            lang: "en".into(),
        };
        frame::send(&mut self.to, &h).unwrap();
        frame::recv(&mut self.from).unwrap().expect("a reply")
    }

    fn up(side: &std::path::Path) -> Host {
        let mut h = Host::start(ServeOptions::default());
        assert!(matches!(h.hello(vec![1], side), HelloReply::Ack { .. }));
        h
    }

    fn send(&mut self, m: &ToChild) {
        frame::send(&mut self.to, m).unwrap();
    }

    fn next(&mut self) -> FromChild {
        frame::recv(&mut self.from).unwrap().expect("an answer")
    }

    fn rid(&mut self) -> u64 {
        self.rid += 1;
        self.rid
    }

    fn cx() -> WCx {
        let settings = Settings::new();
        let scope = ReadScope::unrestricted();
        cx_to_wire(&CxSource {
            settings: &settings,
            lang: "en",
            term: TermCaps::new(),
            external: false,
            read_scope: &scope,
        })
        .unwrap()
    }

    fn open(&mut self, sid: u64, path: &str) -> FromChild {
        let rid = self.rid();
        let input = WInput::File {
            path: path_to_wire(std::path::Path::new(path)),
            hint: None,
        };
        self.send(&ToChild::Open {
            sid,
            rid,
            input,
            cx: Host::cx(),
        });
        self.next()
    }

    fn view_req(&mut self, sid: u64, repr: &str) -> u64 {
        let rid = self.rid();
        let req = WViewReq {
            repr: Some(repr.into()),
            page: None,
            cols: 10,
            rows: 4,
            cell_px: None,
        };
        self.send(&ToChild::View {
            sid,
            rid,
            req,
            cx: Host::cx(),
        });
        rid
    }

    fn finish(mut self) -> std::io::Result<()> {
        drop(self.to);
        self.child.take().unwrap().join().unwrap()
    }
}

fn err_kind(a: &FromChild) -> WFailureKind {
    match &a.answer {
        Answer::Err(f) => f.kind,
        other => panic!("not a failure: {other:?}"),
    }
}

#[test]
fn a_host_that_speaks_no_known_version_is_refused_and_the_child_ends_cleanly() {
    let dir = unique_tmp("kp_srv_ver");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    for versions in [vec![2], vec![0], vec![], vec![99, 100]] {
        let mut h = Host::start(ServeOptions::default());
        match h.hello(versions.clone(), dir.as_path()) {
            HelloReply::Refused { supported, reason } => {
                assert_eq!(supported, vec![PROTOCOL_VERSION]);
                assert!(!reason.is_empty());
            }
            other => panic!("{versions:?}: {other:?}"),
        }
        h.finish().unwrap();
    }
}

#[test]
fn a_host_that_speaks_the_version_among_others_gets_the_manifest() {
    let dir = unique_tmp("kp_srv_ack");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::start(ServeOptions::default());
    match h.hello(vec![0, 1, 99], dir.as_path()) {
        HelloReply::Ack { version, manifest } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert_eq!(manifest_from_wire(manifest).unwrap().id, "test");
        }
        other => panic!("{other:?}"),
    }
    h.finish().unwrap();
}

#[test]
fn a_relative_side_directory_is_refused_by_the_child() {
    let mut h = Host::start(ServeOptions::default());
    let _ = h.hello(vec![1], std::path::Path::new("relative/dir"));
    let r = h.child.take().unwrap().join().unwrap();
    assert!(
        r.is_err(),
        "the child ends with an error instead of using a relative path"
    );
}

#[test]
fn a_call_on_a_session_that_is_not_open_is_corrupt_and_the_child_goes_on() {
    let dir = unique_tmp("kp_srv_sid");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    let rid = h.view_req(404, "plain");
    let a = h.next();
    assert_eq!(a.rid, rid);
    assert!(matches!(err_kind(&a), WFailureKind::Corrupt));
    let rid = h.rid();
    h.send(&ToChild::Act {
        sid: 404,
        rid,
        action: "flash".into(),
        cx: Host::cx(),
    });
    assert!(matches!(err_kind(&h.next()), WFailureKind::Corrupt));
    let rid = h.rid();
    h.send(&ToChild::Key {
        sid: 404,
        rid,
        key: WKey {
            code: WKeyCode::Enter,
            mods: 0,
        },
        cx: Host::cx(),
    });
    assert!(matches!(err_kind(&h.next()), WFailureKind::Corrupt));
    // Closing one that is not open is harmless, and a good open still works.
    h.send(&ToChild::Close { sid: 404 });
    assert!(matches!(
        h.open(1, "/a.t").answer,
        Answer::Ok(Reply::Opened { .. })
    ));
    h.finish().unwrap();
}

#[test]
fn the_session_cap_answers_too_heavy_and_a_close_frees_a_place() {
    let dir = unique_tmp("kp_srv_cap");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::start(ServeOptions {
        max_sessions: 2,
        side_threshold: None,
    });
    assert!(matches!(
        h.hello(vec![1], dir.as_path()),
        HelloReply::Ack { .. }
    ));
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    assert!(matches!(h.open(2, "/a.t").answer, Answer::Ok(_)));
    assert!(matches!(
        err_kind(&h.open(3, "/a.t")),
        WFailureKind::TooHeavy
    ));
    h.send(&ToChild::Close { sid: 1 });
    assert!(matches!(h.open(3, "/a.t").answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn the_same_session_id_can_be_opened_again_at_the_cap() {
    let dir = unique_tmp("kp_srv_reopen");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::start(ServeOptions {
        max_sessions: 1,
        side_threshold: None,
    });
    assert!(matches!(
        h.hello(vec![1], dir.as_path()),
        HelloReply::Ack { .. }
    ));
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    assert!(matches!(h.open(1, "/b.t").answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn a_broken_frame_after_the_handshake_ends_the_child_with_an_error() {
    use std::io::Write;
    let dir = unique_tmp("kp_srv_broken");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    for bytes in [
        vec![0, 0, 0, 2, 0xc1, 0xc1],
        vec![0xff, 0xff, 0xff, 0xff],
        vec![0, 0, 0, 9, 1],
    ] {
        let mut h = Host::up(dir.as_path());
        h.to.write_all(&bytes).unwrap();
        // A cut frame needs the stream closed to be seen as cut.
        let _ = h.to.shutdown(std::net::Shutdown::Write);
        let r = h.child.take().unwrap().join().unwrap();
        assert!(r.is_err(), "{bytes:?}");
    }
}

#[test]
fn shutdown_and_a_closed_stream_end_the_child_cleanly() {
    let dir = unique_tmp("kp_srv_end");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    h.send(&ToChild::Shutdown);
    assert!(h.child.take().unwrap().join().unwrap().is_ok());
    Host::up(dir.as_path()).finish().unwrap();
}

#[test]
fn a_cancel_for_a_call_that_is_not_running_is_harmless() {
    let dir = unique_tmp("kp_srv_cancel_none");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    h.send(&ToChild::Cancel { rid: 12345 });
    h.send(&ToChild::Cancel { rid: 0 });
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn a_cancel_is_seen_while_a_call_runs() {
    let dir = unique_tmp("kp_srv_cancel");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let rid = h.view_req(1, "wait-cancel");
    h.ctl.started.wait();
    h.send(&ToChild::Cancel { rid });
    let a = h.next();
    assert_eq!(a.rid, rid);
    assert!(matches!(err_kind(&a), WFailureKind::Cancelled));
    // The next call is not cancelled.
    let rid = h.view_req(1, "plain");
    let a = h.next();
    assert_eq!(a.rid, rid);
    assert!(matches!(a.answer, Answer::Ok(Reply::Viewed { .. })));
    h.finish().unwrap();
}

#[test]
fn a_cancel_names_a_call_and_leaves_the_others_alone() {
    let dir = unique_tmp("kp_srv_cancel_one");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let first = h.view_req(1, "wait-cancel");
    let second = h.view_req(1, "plain"); // queued behind
    h.ctl.started.wait();
    h.send(&ToChild::Cancel { rid: second }); // not the running one
    h.send(&ToChild::Cancel { rid: first });
    let a = h.next();
    assert_eq!(a.rid, first);
    assert!(matches!(err_kind(&a), WFailureKind::Cancelled));
    let b = h.next();
    assert_eq!(b.rid, second);
    // The second was cancelled before it ran; either answer is the plugin's, but it is an answer.
    assert!(matches!(b.answer, Answer::Ok(_) | Answer::Err(_)));
    h.finish().unwrap();
}

#[test]
fn a_panic_is_a_failure_and_the_loop_goes_on() {
    let dir = unique_tmp("kp_srv_panic");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    let a = h.open(1, "/x/panic-open");
    assert!(matches!(err_kind(&a), WFailureKind::External));
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let rid = h.view_req(1, "panic");
    let a = h.next();
    assert_eq!(a.rid, rid);
    match a.answer {
        Answer::Err(f) => assert!(f.detail.unwrap().contains("panic in view")),
        other => panic!("{other:?}"),
    }
    let _ = h.view_req(1, "plain");
    assert!(matches!(h.next().answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn an_answer_over_the_frame_limit_becomes_a_too_large_failure() {
    let dir = unique_tmp("kp_srv_huge");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let rid = h.view_req(1, "huge");
    let a = h.next();
    assert_eq!(a.rid, rid);
    assert!(matches!(err_kind(&a), WFailureKind::TooLarge));
    let _ = h.view_req(1, "plain");
    assert!(matches!(h.next().answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn hostile_requests_fail_the_call_and_not_the_child() {
    let dir = unique_tmp("kp_srv_hostile");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());

    // A relative path in the input.
    let rid = h.rid();
    let input = WInput::File {
        path: path_to_wire(std::path::Path::new("rel.t")),
        hint: None,
    };
    h.send(&ToChild::Open {
        sid: 1,
        rid,
        input,
        cx: Host::cx(),
    });
    assert!(matches!(h.next().answer, Answer::Err(_)));

    // A relative read scope.
    let mut cx = Host::cx();
    cx.read_dirs = Some(vec![path_to_wire(std::path::Path::new("rel"))]);
    let rid = h.rid();
    let input = WInput::File {
        path: path_to_wire(std::path::Path::new("/a.t")),
        hint: None,
    };
    h.send(&ToChild::Open {
        sid: 1,
        rid,
        input,
        cx,
    });
    assert!(matches!(h.next().answer, Answer::Err(_)));

    // A side file that is not there.
    let rid = h.rid();
    let input = WInput::Content {
        content_type: "text/plain".into(),
        text: "x".into(),
        resources: vec![(
            "r".into(),
            WBin::File {
                name: "h-404".into(),
                len: 5,
            },
        )],
        origin: WOrigin::File(path_to_wire(std::path::Path::new("/o.md"))),
    };
    h.send(&ToChild::Open {
        sid: 1,
        rid,
        input,
        cx: Host::cx(),
    });
    assert!(matches!(h.next().answer, Answer::Err(_)));

    // A side file whose name climbs out of the directory.
    let rid = h.rid();
    let input = WInput::Content {
        content_type: "text/plain".into(),
        text: "x".into(),
        resources: vec![(
            "r".into(),
            WBin::File {
                name: "../secret".into(),
                len: 5,
            },
        )],
        origin: WOrigin::File(path_to_wire(std::path::Path::new("/o.md"))),
    };
    h.send(&ToChild::Open {
        sid: 1,
        rid,
        input,
        cx: Host::cx(),
    });
    assert!(matches!(h.next().answer, Answer::Err(_)));

    // A view request with a zero-size window and one with a huge page.
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let rid = h.rid();
    let req = WViewReq {
        repr: None,
        page: Some(u64::MAX),
        cols: 0,
        rows: 0,
        cell_px: Some((0, 0)),
    };
    h.send(&ToChild::View {
        sid: 1,
        rid,
        req,
        cx: Host::cx(),
    });
    let _ = h.next(); // an answer of some kind; the point is that the child is still here

    let _ = h.view_req(1, "plain");
    assert!(matches!(h.next().answer, Answer::Ok(_)));
    h.finish().unwrap();
}

#[test]
fn every_answer_carries_the_layout_and_the_enabled_state_of_each_declared_action() {
    let dir = unique_tmp("kp_srv_state");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    let declared: Vec<String> = test_manifest("test")
        .actions
        .iter()
        .map(|a| a.id.clone())
        .collect();
    let Answer::Ok(Reply::Opened { state }) = h.open(1, "/a.t").answer else {
        panic!()
    };
    let ids: Vec<&str> = state.enabled.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(ids, declared.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(state.enabled.iter().any(|(k, v)| k == "gated" && !v));
    let rid = h.rid();
    h.send(&ToChild::Act {
        sid: 1,
        rid,
        action: "toggle".into(),
        cx: Host::cx(),
    });
    let Answer::Ok(Reply::Acted { state, .. }) = h.next().answer else {
        panic!()
    };
    assert!(state.enabled.iter().any(|(k, v)| k == "gated" && *v));
    assert_eq!(state.layout.reprs.len(), 2);
    h.finish().unwrap();
}

#[test]
fn a_big_byte_string_goes_through_a_side_file_named_c_and_stays_inside_the_directory() {
    let dir = unique_tmp("kp_srv_side");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut h = Host::up(dir.as_path());
    assert!(matches!(h.open(1, "/a.t").answer, Answer::Ok(_)));
    let rid = h.view_req(1, "bigdoc");
    let a = h.next();
    assert_eq!(a.rid, rid);
    let Answer::Ok(Reply::Viewed {
        view: WView::Document(d),
        ..
    }) = a.answer
    else {
        panic!("{a:?}")
    };
    let named: Vec<&str> = d
        .resources
        .iter()
        .filter_map(|(_, b)| match b {
            WBin::File { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(named.len(), 1, "the 2 MiB resource went through a file");
    assert!(named[0].starts_with("c-"), "{}", named[0]);
    assert!(
        dir.as_path().join(named[0]).is_file(),
        "the file waits for the host to read it"
    );
    h.finish().unwrap();
}

#[test]
fn eof_before_the_hello_is_a_clean_end() {
    let (to, child_r) = UnixStream::pair().unwrap();
    let (child_w, _from) = UnixStream::pair().unwrap();
    let ctl = Arc::new(Ctl::default());
    let t = std::thread::spawn(move || {
        serve_io(
            TestPlugin { ctl },
            child_r,
            child_w,
            &ServeOptions::default(),
        )
    });
    drop(to);
    assert!(t.join().unwrap().is_ok());
}

#[test]
fn a_bad_hello_is_an_error() {
    use std::io::Write;
    let (mut to, child_r) = UnixStream::pair().unwrap();
    let (child_w, _from) = UnixStream::pair().unwrap();
    let ctl = Arc::new(Ctl::default());
    let t = std::thread::spawn(move || {
        serve_io(
            TestPlugin { ctl },
            child_r,
            child_w,
            &ServeOptions::default(),
        )
    });
    to.write_all(&[0, 0, 0, 2, 0xc1, 0xc1]).unwrap();
    assert!(t.join().unwrap().is_err());
    let _ = Duration::from_secs(0);
}
