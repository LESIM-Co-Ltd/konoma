//! The tests. Every one that starts a child waits for it, and checks that no process of its group
//! is left (`gone`).

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::proc::client::{CommandLaunch, Config, Launch, Limits, ProcPlugin, Spawned};
use konoma_plugin::proc::frame;
use konoma_plugin::proc::manifest_from_json;
use konoma_plugin::proc::proto::*;
use konoma_plugin::proc::wire::*;
use konoma_plugin::view::*;
use konoma_plugin::*;
use konoma_test_support::unique_tmp;

use crate::child::CHILD_ENV;

pub fn all() -> Vec<(&'static str, fn())> {
    vec![
        (
            "command_launch_gives_the_child_its_own_group_and_no_core_dump",
            launch_group_rlimit,
        ),
        (
            "stop_kills_the_group_and_waits_for_the_child",
            stop_kills_and_waits,
        ),
        (
            "a_stray_println_goes_to_stderr_and_the_protocol_survives",
            println_goes_to_stderr,
        ),
        ("the_command_line_modes_print_and_refuse", cli_modes),
        (
            "an_argument_that_is_not_utf8_is_refused_without_a_panic",
            non_utf8_argument,
        ),
        (
            "a_closed_stdout_does_not_make_manifest_panic",
            manifest_to_a_closed_pipe,
        ),
        (
            "an_idle_child_exits_at_once_when_its_host_goes",
            host_gone_idle,
        ),
        (
            "a_child_stuck_in_a_call_exits_after_the_grace_when_its_host_goes",
            host_gone_busy,
        ),
        (
            "shutdown_ends_the_child_cleanly_after_dropping_its_sessions",
            shutdown_drops_sessions,
        ),
        (
            "a_panic_in_a_session_drop_does_not_take_the_child_down",
            drop_panic_is_contained,
        ),
        (
            "the_protocol_pipes_do_not_reach_a_grandchild",
            no_fd_leak_to_a_grandchild,
        ),
        ("the_memory_watch_ends_a_child_over_the_limit", rss_watch),
    ]
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The child's wait for a host that left (`HOST_GONE_GRACE` of the SDK).
const HOST_GONE_GRACE: Duration = Duration::from_secs(2);

fn exe() -> PathBuf {
    std::env::current_exe().expect("current_exe")
}

fn child_command() -> Command {
    let mut c = Command::new(exe());
    c.env(CHILD_ENV, "1");
    c
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Neither the process nor anything of its process group exists (after a reap).
fn gone(pid: u32) {
    let pid = pid as i32;
    assert!(!alive(pid), "process {pid} is still there");
    assert!(!alive(-pid), "the process group {pid} is not empty");
}

fn until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never happened: {what}");
}

fn wait_exit(child: &mut Child, secs: u64) -> std::process::ExitStatus {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Some(s) = child.try_wait().expect("try_wait") {
            return s;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the child did not exit within {secs} s");
}

/// A launcher that remembers the pid of every child it starts (and uses the real `CommandLaunch`).
struct Recording {
    inner: CommandLaunch,
    pids: Arc<Mutex<Vec<u32>>>,
}

impl Launch for Recording {
    fn launch(&self) -> std::io::Result<Spawned> {
        let s = self.inner.launch()?;
        if let Some(p) = s.pid {
            self.pids.lock().unwrap().push(p);
        }
        Ok(s)
    }
}

fn plugin(limits: Limits, scratch: &Path, extra: &[&str]) -> (ProcPlugin, Arc<Mutex<Vec<u32>>>) {
    std::env::set_var(CHILD_ENV, "1");
    let pids = Arc::new(Mutex::new(Vec::new()));
    let mut args = vec!["--serve"];
    args.extend_from_slice(extra);
    let launch = Recording {
        inner: CommandLaunch::new(exe(), args),
        pids: Arc::clone(&pids),
    };
    let cfg = Config {
        limits,
        scratch_root: scratch.to_path_buf(),
        host_version: "0.0.0-test".into(),
        lang: "en".into(),
    };
    (
        ProcPlugin::start(launch, cfg).expect("the child starts"),
        pids,
    )
}

fn limits() -> Limits {
    Limits {
        open_view: Duration::from_secs(20),
        act_key: Duration::from_secs(20),
        cancel_grace: Duration::from_secs(5),
        start_timeout: Duration::from_secs(20),
        shutdown_grace: Duration::from_secs(10),
        rss_limit: u64::MAX,
        ..Limits::default()
    }
}

fn open(p: &ProcPlugin, dir: &Path, name: &str) -> Box<dyn Session> {
    p.open(
        Input::File {
            path: dir.join(name),
            hint: None,
        },
        &OpenCx::bare(),
    )
    .expect("open")
}

fn flash(s: &mut Box<dyn Session>, action: &str) -> String {
    match s.act(action, &ActCx::bare()) {
        Ok(Outcome::Flash(t)) => t.en().to_string(),
        other => panic!("not a flash: {other:?}"),
    }
}

fn field(text: &str, key: &str) -> i32 {
    text.split_whitespace()
        .find_map(|w| w.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no {key} in {text:?}"))
}

fn the_pid(pids: &Arc<Mutex<Vec<u32>>>) -> u32 {
    let v = pids.lock().unwrap();
    assert_eq!(v.len(), 1, "one child was started");
    v[0]
}

// ---------------------------------------------------------------------------------------------
// CommandLaunch, kill and wait
// ---------------------------------------------------------------------------------------------

fn launch_group_rlimit() {
    let dir = unique_tmp("kpp_group");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let (p, pids) = plugin(limits(), dir.as_path(), &[]);
    let mut s = open(&p, dir.as_path(), "a.t");
    let info = flash(&mut s, "info");
    let pid = the_pid(&pids);
    assert_eq!(field(&info, "pid"), pid as i32, "{info}");
    assert_eq!(
        field(&info, "pgid"),
        pid as i32,
        "the child leads its own group: {info}"
    );
    assert_ne!(
        field(&info, "pgid"),
        unsafe { libc::getpgrp() },
        "and it is not the host's group"
    );
    assert_eq!(field(&info, "core"), 0, "no core dump: {info}");
    drop(s);
    p.stop();
    gone(pid);
}

fn stop_kills_and_waits() {
    let dir = unique_tmp("kpp_stop");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let (p, pids) = plugin(limits(), dir.as_path(), &[]);
    let mut s = open(&p, dir.as_path(), "a.t");
    let pid = the_pid(&pids);
    let gc = field(&flash(&mut s, "grandchild"), "grandchild");
    assert!(alive(gc), "the grandchild runs");
    // The child leaves on its own at `stop`, but the grandchild would stay. The group is killed.
    p.stop();
    gone(pid);
    until("the grandchild was killed with the group", 10, || {
        !alive(gc)
    });
    // The session is dead afterwards.
    assert!(s.act("info", &ActCx::bare()).is_err());
}

// ---------------------------------------------------------------------------------------------
// serve: fd rewiring
// ---------------------------------------------------------------------------------------------

fn println_goes_to_stderr() {
    let dir = unique_tmp("kpp_println");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let (p, pids) = plugin(limits(), dir.as_path(), &[]);
    let mut s = open(&p, dir.as_path(), "a.t");
    let v = s
        .view(&ViewReq::new(10, 3).repr("print"), &ViewCx::bare())
        .expect("a frame was not corrupted by the plugin's println!");
    assert!(
        matches!(&v, View::Text(t) if matches!(&t.source, TextSource::Owned(x) if x == "printed"))
    );
    // The session still works after it.
    assert!(flash(&mut s, "info").contains("pid="));
    // The line went to stderr: the host sees it in the detail of the failure of the next crash.
    let err = s.act("exit", &ActCx::bare()).expect_err("the child exits");
    let detail = err.detail.clone().unwrap_or_default();
    assert!(detail.contains("STRAY-STDOUT-LINE"), "{detail:?}");
    assert!(detail.contains("STRAY-BEFORE-EXIT"), "{detail:?}");
    assert!(detail.contains("LAST-WORDS"), "{detail:?}");
    p.stop();
    gone(the_pid(&pids));
}

// ---------------------------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------------------------

fn cli_modes() {
    let run = |arg: &str| {
        child_command()
            .arg(arg)
            .stdin(Stdio::null())
            .output()
            .expect("run")
    };
    let m = run("--manifest");
    assert!(m.status.success());
    let json = String::from_utf8(m.stdout).unwrap();
    let manifest = manifest_from_json(&json).expect("the output reads back");
    assert_eq!(manifest.id, "proc-test");
    let v = run("--version");
    assert!(v.status.success());
    assert_eq!(
        String::from_utf8(v.stdout).unwrap().trim(),
        "proc-test 1.2.3"
    );
    for h in ["--help", "-h"] {
        let o = run(h);
        assert!(o.status.success(), "{h}");
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("--serve"),
            "{h}"
        );
    }
    let unknown = run("--nope");
    assert_eq!(unknown.status.code(), Some(2));
    let none = child_command().stdin(Stdio::null()).output().unwrap();
    assert_eq!(none.status.code(), Some(2));
}

fn non_utf8_argument() {
    let bad = OsString::from_vec(vec![b'f', 0xff, 0xfe, b'.', b't']);
    let o = child_command()
        .arg(&bad)
        .stdin(Stdio::null())
        .output()
        .expect("run");
    // A refusal, not the 101 of a panic.
    assert_eq!(
        o.status.code(),
        Some(2),
        "{:?}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
    // And one that follows an option.
    let o = child_command()
        .arg("--nope")
        .arg(&bad)
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(o.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
}

fn manifest_to_a_closed_pipe() {
    let mut c = child_command()
        .arg("--manifest")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Close the reading end before the child writes (it is still starting up).
    drop(c.stdout.take());
    let mut err = String::new();
    c.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    let s = wait_exit(&mut c, 20);
    assert_ne!(s.code(), Some(101), "a panic: {err}");
    assert!(!err.contains("panicked"), "{err}");
}

// ---------------------------------------------------------------------------------------------
// A raw host over the real pipes
// ---------------------------------------------------------------------------------------------

struct Raw {
    child: Child,
    to: Option<std::process::ChildStdin>,
    from: std::process::ChildStdout,
    rid: u64,
}

impl Raw {
    fn start(dir: &Path) -> Raw {
        let mut child = child_command()
            .arg("--serve")
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn");
        let mut to = child.stdin.take().unwrap();
        let mut from = child.stdout.take().unwrap();
        let hello = Hello {
            versions: vec![PROTOCOL_VERSION],
            host_version: "t".into(),
            side_dir: path_to_wire(dir),
            lang: "en".into(),
        };
        frame::send(&mut to, &hello).unwrap();
        let reply: HelloReply = frame::recv(&mut from).unwrap().expect("a reply");
        assert!(matches!(reply, HelloReply::Ack { .. }), "{reply:?}");
        Raw {
            child,
            to: Some(to),
            from,
            rid: 0,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, m: &ToChild) {
        frame::send(self.to.as_mut().expect("still connected"), m).unwrap();
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

    fn open(&mut self, sid: u64, path: &Path) {
        self.rid += 1;
        let rid = self.rid;
        self.send(&ToChild::Open {
            sid,
            rid,
            input: WInput::File {
                path: path_to_wire(path),
                hint: None,
            },
            cx: Raw::cx(),
        });
        let a: FromChild = frame::recv(&mut self.from).unwrap().expect("an answer");
        assert!(
            matches!(a.answer, Answer::Ok(Reply::Opened { .. })),
            "{a:?}"
        );
    }

    fn act(&mut self, sid: u64, action: &str) -> u64 {
        self.rid += 1;
        let rid = self.rid;
        self.send(&ToChild::Act {
            sid,
            rid,
            action: action.into(),
            cx: Raw::cx(),
        });
        rid
    }

    fn answer(&mut self) -> FromChild {
        frame::recv(&mut self.from).unwrap().expect("an answer")
    }
}

fn host_gone_idle() {
    let dir = unique_tmp("kpp_gone_idle");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut r = Raw::start(dir.as_path());
    let pid = r.pid();
    r.open(1, &dir.as_path().join("a.t"));
    // The host "dies": its end of the pipe closes, with no `Shutdown`.
    drop(r.to.take());
    let t0 = Instant::now();
    let s = wait_exit(&mut r.child, 20);
    assert!(
        t0.elapsed() < HOST_GONE_GRACE + Duration::from_secs(10),
        "left after {:?}",
        t0.elapsed()
    );
    assert!(s.success(), "an idle child leaves cleanly: {s:?}");
    gone(pid);
}

fn host_gone_busy() {
    let dir = unique_tmp("kpp_gone_busy");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut r = Raw::start(dir.as_path());
    let pid = r.pid();
    r.open(1, &dir.as_path().join("a.t"));
    // A call that never looks at its flag; the answer never comes.
    r.act(1, "hang");
    std::thread::sleep(Duration::from_millis(300));
    drop(r.to.take());
    let t0 = Instant::now();
    let s = wait_exit(&mut r.child, 30);
    assert!(
        t0.elapsed() >= HOST_GONE_GRACE - Duration::from_millis(200),
        "it left before the grace, after {:?}",
        t0.elapsed()
    );
    assert_eq!(s.code(), Some(1), "{s:?}");
    gone(pid);
}

fn shutdown_drops_sessions() {
    let dir = unique_tmp("kpp_shutdown");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut r = Raw::start(dir.as_path());
    let pid = r.pid();
    r.open(1, &dir.as_path().join("one.t"));
    r.open(2, &dir.as_path().join("two.t"));
    r.send(&ToChild::Shutdown);
    let s = wait_exit(&mut r.child, 20);
    assert!(s.success(), "{s:?}");
    for n in ["one", "two"] {
        let marks =
            std::fs::read_to_string(dir.as_path().join(format!("{n}.marks"))).unwrap_or_default();
        assert_eq!(
            marks, "drop\n",
            "the session {n} was dropped before the child left"
        );
    }
    gone(pid);
}

fn no_fd_leak_to_a_grandchild() {
    let dir = unique_tmp("kpp_leak");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut r = Raw::start(dir.as_path());
    let pid = r.pid();
    r.open(1, &dir.as_path().join("a.t"));
    let rid = r.act(1, "grandchild");
    let a = r.answer();
    assert_eq!(a.rid, rid);
    let gc = match &a.answer {
        Answer::Ok(Reply::Acted {
            outcome: WOutcome::Flash(t),
            ..
        }) => field(&t.en, "grandchild"),
        other => panic!("{other:?}"),
    };
    assert!(alive(gc));
    r.send(&ToChild::Shutdown);
    // The child leaves; the grandchild goes on. If the grandchild held a copy of the protocol
    // pipe, this read would never see the end of the stream.
    let from = r.from;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut from = from;
        let mut sink = Vec::new();
        let _ = tx.send(from.read_to_end(&mut sink).is_ok());
    });
    let ended = rx.recv_timeout(Duration::from_secs(20));
    let still_alive = alive(gc);
    // Clean up first, whatever the verdict.
    // SAFETY: the group of a child this test started and has not yet reaped.
    unsafe {
        libc::killpg(pid as i32, libc::SIGKILL);
    }
    let _ = r.child.wait();
    if ended.is_ok() {
        reader.join().unwrap();
    }
    assert!(
        ended.is_ok(),
        "the host never saw the end of the pipe: the grandchild holds it"
    );
    assert!(still_alive, "the grandchild was alive when the pipe ended");
    until("the grandchild was killed with the group", 10, || {
        !alive(gc)
    });
    gone(pid);
}

// ---------------------------------------------------------------------------------------------
// The memory watch
// ---------------------------------------------------------------------------------------------

fn rss_watch() {
    let dir = unique_tmp("kpp_rss");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let lim = Limits {
        rss_limit: 150 << 20,
        rss_interval: Duration::from_millis(20),
        shutdown_grace: Duration::from_secs(2),
        ..limits()
    };
    let (p, pids) = plugin(lim, dir.as_path(), &[]);
    let pid = the_pid(&pids);
    if konoma_plugin_rss_unavailable(pid) {
        // No way to read the memory here: the watch is off, which is its documented behaviour.
        p.stop();
        gone(pid);
        return;
    }
    let mut s = open(&p, dir.as_path(), "a.t");
    let f = s
        .act("alloc", &ActCx::bare())
        .expect_err("the child was ended for its memory");
    assert_eq!(f.kind, FailureKind::TooHeavy, "{f:?}");
    assert!(f.detail.unwrap_or_default().contains("MiB"));
    until("the child is gone", 10, || !alive(pid as i32));
    p.stop();
    gone(pid);
}

/// Whether the memory of a process cannot be read on this platform (the watch is then off).
fn konoma_plugin_rss_unavailable(_pid: u32) -> bool {
    !cfg!(any(target_os = "linux", target_os = "macos"))
}

// ---------------------------------------------------------------------------------------------
// A panic in the plugin's Drop
// ---------------------------------------------------------------------------------------------

fn drop_panic_is_contained() {
    let dir = unique_tmp("kpp_dropbomb");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut r = Raw::start(dir.as_path());
    let pid = r.pid();
    r.open(1, &dir.as_path().join("bomb.t"));
    r.open(2, &dir.as_path().join("two.t"));
    r.open(3, &dir.as_path().join("bomb.t"));
    // Closing the first one panics in its Drop; the child and the other sessions go on.
    r.send(&ToChild::Close { sid: 1 });
    let rid = r.act(2, "info");
    let a = r.answer();
    assert_eq!(a.rid, rid);
    assert!(matches!(a.answer, Answer::Ok(Reply::Acted { .. })), "{a:?}");
    assert!(alive(pid as i32), "the child is up after the panic");
    // The `Shutdown` drops the rest, a second panicking Drop among them, and the child still
    // leaves cleanly (not by an abort or a 101).
    r.send(&ToChild::Shutdown);
    let s = wait_exit(&mut r.child, 20);
    assert!(s.success(), "{s:?}");
    let marks = std::fs::read_to_string(dir.as_path().join("bomb.marks")).unwrap_or_default();
    assert_eq!(marks, "drop\ndrop\n", "both bombs were dropped");
    assert_eq!(
        std::fs::read_to_string(dir.as_path().join("two.marks")).unwrap_or_default(),
        "drop\n"
    );
    gone(pid);
}
