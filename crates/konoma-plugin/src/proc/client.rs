//! The host side: a plugin that lives in a child process.
//!
//! [`ProcPlugin`] implements [`Plugin`], so the host handles it like an in-process plugin. The child
//! is started on first use and serves every session of the plugin, one call at a time in order;
//! the host side multiplexes by request id.
//!
//! # When the child ends
//!
//! A call past its deadline, a child that dies, a frame that is broken or over the limit, an
//! answer for a request nobody made, and a child over its memory limit all end the **whole child**
//! (its process group is killed). Every call in flight fails, every session of that child fails
//! from then on, and the next `open` starts a new child. The failure carries the last bytes of the
//! child's stderr as its detail (`Limits::stderr_tail`).
//!
//! An answer the host cannot convert (a canvas that fails its check, a relative picture path) fails
//! only that call: the child is still talking sense.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::frame::{self, FrameError};
use super::proto::{
    host_versions, Answer, FromChild, Hello, HelloReply, Reply, ToChild, WState, PROTOCOL_VERSION,
};
use super::wire::{
    self, cx_to_wire, failure_from_wire, input_to_wire, key_to_wire, layout_from_wire,
    manifest_from_wire, outcome_from_wire, path_to_wire, req_to_wire, view_from_wire, CxSource,
    Dec, Enc, Side,
};
use crate::session::{ActCx, OpenCx, ViewCx};
use crate::view::Layout;
use crate::{t, Failure, FailureKind, Input, Key, Manifest, Outcome, Plugin, Session, Tr};
use crate::{View, ViewReq};

/// The deadlines and limits of a connection. The defaults are the product's; tests shorten them.
#[derive(Clone, Debug)]
pub struct Limits {
    /// How long `open` and `view` may take. 30 s: opening a large workbook or PDF is the slowest
    /// honest thing a plugin does, and the user can cancel earlier; the same number is the
    /// start-up limit of the SVG child (`START_LIMIT`).
    pub open_view: Duration,
    /// How long `act` and `key` may take. 10 s: they change state or answer a keystroke, so a
    /// longer wait is a hang, not work.
    pub act_key: Duration,
    /// After the host asked a call to stop (the `Cancel` of its context went up), how long the
    /// plugin has to answer before the child is killed. 2 s: a plugin that checks its flag at all
    /// checks it far more often.
    pub cancel_grace: Duration,
    /// A child with no live session and no call for this long is stopped by `reap_idle`. 5 min:
    /// long enough that a user going back and forth between files does not pay the start-up each
    /// time, short enough that a plugin the user has stopped using does not stay in memory.
    pub idle_after: Duration,
    /// How long the child has to answer the handshake. 30 s, as for `open_view` (a plugin that
    /// loads a large model at start-up is slow here, not hung).
    pub start_timeout: Duration,
    /// How much of the end of stderr is kept for a failure's detail.
    pub stderr_tail: usize,
    /// The resident memory at which the child is killed, in bytes. 2 GiB: the SVG child gets 1 GiB
    /// for one drawing; a plugin serves many sessions and opens whole documents.
    pub rss_limit: u64,
    /// How often the memory is read.
    pub rss_interval: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            open_view: Duration::from_secs(30),
            act_key: Duration::from_secs(10),
            cancel_grace: Duration::from_secs(2),
            idle_after: Duration::from_secs(5 * 60),
            start_timeout: Duration::from_secs(30),
            stderr_tail: 4096,
            rss_limit: 2 << 30,
            rss_interval: Duration::from_millis(100),
        }
    }
}

/// Everything about a [`ProcPlugin`] besides how its child is started.
#[derive(Clone, Debug)]
pub struct Config {
    pub limits: Limits,
    /// Where the per-connection handoff directory is created (default: the system temp directory).
    pub scratch_root: PathBuf,
    /// Sent in the handshake.
    pub host_version: String,
    /// Sent in the handshake.
    pub lang: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            limits: Limits::default(),
            scratch_root: std::env::temp_dir(),
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            lang: "en".to_string(),
        }
    }
}

/// How often a waiting call looks at its deadline and its cancel flag. The cancel flag is an opaque
/// closure, so it can only be polled.
const CALL_POLL: Duration = Duration::from_millis(10);
/// How long a failing connection waits for the end of the child's stderr before it reads the tail.
const STDERR_JOIN: Duration = Duration::from_millis(300);

/// The pieces of a started child.
pub struct Spawned {
    /// The child's protocol output.
    pub reader: Box<dyn Read + Send>,
    /// The child's protocol input.
    pub writer: Box<dyn Write + Send>,
    /// The child's stderr, if it has one the host can read.
    pub stderr: Option<Box<dyn Read + Send>>,
    /// Kill the child and everything it started, and wait for it. Called more than once.
    pub kill: Box<dyn Fn() + Send + Sync>,
    /// For the memory watch.
    pub pid: Option<u32>,
}

/// How a child is started.
pub trait Launch: Send + Sync {
    fn launch(&self) -> std::io::Result<Spawned>;
}

impl<F> Launch for F
where
    F: Fn() -> std::io::Result<Spawned> + Send + Sync,
{
    fn launch(&self) -> std::io::Result<Spawned> {
        self()
    }
}

/// Start a program: its own process group, no core dump, working directory `/`.
pub struct CommandLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl CommandLaunch {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        CommandLaunch {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

impl Launch for CommandLaunch {
    fn launch(&self) -> std::io::Result<Spawned> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: only async-signal-safe calls (`setrlimit`) between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                let none = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::setrlimit(libc::RLIMIT_CORE, &none);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
            kill_group(&Mutex::new(Some(child)), pid);
            return Err(std::io::Error::other("the child's pipes are missing"));
        };
        let child = Mutex::new(Some(child));
        Ok(Spawned {
            reader: Box::new(stdout),
            writer: Box::new(stdin),
            stderr: Some(Box::new(stderr)),
            kill: Box::new(move || kill_group(&child, pid)),
            pid: Some(pid),
        })
    }
}

/// Kill the child's process group, then reap the child. The pid is only used while the child is
/// still unreaped (a zombie keeps its pid), so it cannot name another process.
fn kill_group(child: &Mutex<Option<Child>>, pid: u32) {
    let mut guard = child.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut c) = guard.take() {
        // SAFETY: plain syscall on the group of a child this process started and has not reaped.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// One connection
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct SessionState {
    layout: Layout,
    enabled: HashMap<String, bool>,
}

enum Done {
    Opened(SessionState),
    Viewed(View, SessionState),
    Acted(Outcome, SessionState),
}

type CallResult = Result<Done, Failure>;

struct State {
    /// Set once, when the connection ends; the first reason stays.
    dead: Option<Failure>,
    pending: HashMap<u64, mpsc::Sender<CallResult>>,
}

struct Shared {
    limits: Limits,
    side: Side,
    side_dir: PathBuf,
    state: Mutex<State>,
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    kill: Box<dyn Fn() + Send + Sync>,
    pid: Option<u32>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    stderr_done: Arc<(Mutex<bool>, Condvar)>,
    /// Set by the thread that is ending the connection (see `fail`).
    ending: AtomicBool,
    next_rid: AtomicU64,
    /// Sessions open plus opens in flight.
    live: AtomicUsize,
    last_used: Mutex<Instant>,
    /// Wakes the memory watch when the connection ends.
    stop: (Mutex<bool>, Condvar),
}

impl Shared {
    fn tail_text(&self) -> Option<String> {
        let tail = lock(&self.stderr_tail);
        if tail.is_empty() {
            return None;
        }
        let bytes: Vec<u8> = tail.iter().copied().collect();
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn dead_failure(&self) -> Option<Failure> {
        lock(&self.state).dead.clone()
    }

    fn is_dead(&self) -> bool {
        lock(&self.state).dead.is_some()
    }

    /// End the connection: kill the child, record why, fail everything waiting. The first reason
    /// is kept; later calls change nothing. Returns the reason of record.
    fn fail(&self, kind: FailureKind, message: Tr, detail: Option<String>) -> Failure {
        if let Some(f) = self.dead_failure() {
            return f;
        }
        if self.ending.swap(true, Ordering::SeqCst) {
            // Another thread is ending the connection; the reason of record is its, not the
            // child's exit that our own kill causes. Wait for it (it is bounded by the stderr
            // join below).
            let until = Instant::now() + STDERR_JOIN * 4;
            while Instant::now() < until {
                if let Some(f) = self.dead_failure() {
                    return f;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            return self
                .dead_failure()
                .unwrap_or_else(|| Failure::new(kind, message));
        }
        (self.kill)();
        {
            let (done, cv) = &*self.stderr_done;
            let mut d = lock(done);
            let until = Instant::now() + STDERR_JOIN;
            while !*d {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                d = cv
                    .wait_timeout(d, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        let mut failure = Failure::new(kind, message);
        failure.detail = self.tail_text().or(detail);
        let pending = {
            let mut st = lock(&self.state);
            if let Some(f) = &st.dead {
                return f.clone();
            }
            st.dead = Some(failure.clone());
            std::mem::take(&mut st.pending)
        };
        *lock(&self.tx) = None;
        for (_, tx) in pending {
            let _ = tx.send(Err(failure.clone()));
        }
        {
            let (flag, cv) = &self.stop;
            *lock(flag) = true;
            cv.notify_all();
        }
        let _ = std::fs::remove_dir_all(&self.side_dir);
        failure
    }

    fn died(&self, why: &str) -> Failure {
        self.fail(
            FailureKind::External,
            t!(
                en: "the plugin stopped working",
                ja: "プラグインが動かなくなりました"
            ),
            Some(why.to_string()),
        )
    }

    fn send_frame(&self, msg: &ToChild) -> Result<(), Failure> {
        let body = frame::encode(msg).map_err(|e| {
            Failure::too_large(t!(
                en: "the request is too large to send to the plugin",
                ja: "プラグインへの要求が大きすぎます"
            ))
            .with_detail(e.to_string())
        })?;
        let mut buf = Vec::with_capacity(body.len() + 4);
        buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
        buf.extend_from_slice(&body);
        let sent = lock(&self.tx).as_ref().map(|tx| tx.send(buf));
        match sent {
            Some(Ok(())) => Ok(()),
            _ => Err(self
                .dead_failure()
                .unwrap_or_else(|| self.died("the connection to the plugin is closed"))),
        }
    }

    /// Send `build(rid)` and wait for its answer: until `deadline`, and, once `cancel` goes up,
    /// at most `cancel_grace` more. Past either, the child is ended.
    fn call(
        &self,
        build: impl FnOnce(u64) -> Result<ToChild, Failure>,
        deadline: Duration,
        cancel: &crate::Cancel,
    ) -> CallResult {
        let rid = self.next_rid.fetch_add(1, Ordering::Relaxed);
        let msg = build(rid)?;
        let (tx, rx) = mpsc::channel();
        {
            let mut st = lock(&self.state);
            if let Some(f) = &st.dead {
                return Err(f.clone());
            }
            st.pending.insert(rid, tx);
        }
        let started = Instant::now();
        let result = self.wait(rid, &msg, rx, started, deadline, cancel);
        lock(&self.state).pending.remove(&rid);
        *lock(&self.last_used) = Instant::now();
        result
    }

    fn wait(
        &self,
        rid: u64,
        msg: &ToChild,
        rx: mpsc::Receiver<CallResult>,
        started: Instant,
        deadline: Duration,
        cancel: &crate::Cancel,
    ) -> CallResult {
        self.send_frame(msg)?;
        let mut cancel_at: Option<Instant> = None;
        loop {
            match rx.recv_timeout(CALL_POLL) {
                Ok(res) => return res,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self
                        .dead_failure()
                        .unwrap_or_else(|| self.died("the call was dropped")));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if now.duration_since(started) >= deadline {
                return Err(self.fail(
                    FailureKind::Timeout,
                    t!(
                        en: "the plugin did not answer in time",
                        ja: "プラグインが時間内に応答しませんでした"
                    ),
                    Some(format!("no answer within {deadline:?}")),
                ));
            }
            match cancel_at {
                None if cancel.is_cancelled() => {
                    // A failed send is the connection's end; the loop then ends with its reason.
                    let _ = self.send_frame(&ToChild::Cancel { rid });
                    cancel_at = Some(now + self.limits.cancel_grace);
                }
                Some(at) if now >= at => {
                    let f = self.fail(
                        FailureKind::Cancelled,
                        t!(
                            en: "the plugin did not stop when asked and was ended",
                            ja: "中断に応じなかったためプラグインを終了しました"
                        ),
                        Some(format!(
                            "no answer {:?} after the cancel",
                            self.limits.cancel_grace
                        )),
                    );
                    return Err(f);
                }
                _ => {}
            }
        }
    }
}

fn state_from_wire(s: WState) -> SessionState {
    SessionState {
        layout: layout_from_wire(s.layout),
        enabled: s.enabled.into_iter().collect(),
    }
}

/// The reader thread: answers go to the call waiting for them.
fn read_loop(shared: Arc<Shared>, mut reader: Box<dyn Read + Send>) {
    loop {
        let msg: FromChild = match frame::recv(&mut reader) {
            Ok(Some(m)) => m,
            Ok(None) => {
                shared.died("the plugin closed its output (it exited)");
                return;
            }
            Err(e) => {
                shared.died(&e.to_string());
                return;
            }
        };
        let tx = lock(&shared.state).pending.remove(&msg.rid);
        let Some(tx) = tx else {
            shared.died(&format!(
                "an answer to request {}, which nobody made",
                msg.rid
            ));
            return;
        };
        let result = convert(&shared, msg.answer);
        let _ = tx.send(result);
    }
}

fn convert(shared: &Shared, answer: Answer) -> CallResult {
    let mut dec = Dec::new(&shared.side);
    match answer {
        Answer::Err(f) => Err(failure_from_wire(f)),
        Answer::Ok(Reply::Opened { state }) => Ok(Done::Opened(state_from_wire(state))),
        Answer::Ok(Reply::Viewed { view, state }) => {
            let view = view_from_wire(view, &mut dec).map_err(Failure::from)?;
            Ok(Done::Viewed(view, state_from_wire(state)))
        }
        Answer::Ok(Reply::Acted { outcome, state }) => Ok(Done::Acted(
            outcome_from_wire(outcome),
            state_from_wire(state),
        )),
    }
}

/// The memory watch.
fn watch_loop(shared: Arc<Shared>, pid: u32) {
    let (flag, cv) = &shared.stop;
    let mut stopped = lock(flag);
    loop {
        if *stopped {
            return;
        }
        let (g, _) = cv
            .wait_timeout(stopped, shared.limits.rss_interval)
            .unwrap_or_else(|e| e.into_inner());
        stopped = g;
        if *stopped {
            return;
        }
        drop(stopped);
        if let Some(rss) = super::os::rss_bytes(pid) {
            if rss > shared.limits.rss_limit {
                shared.fail(
                    FailureKind::TooHeavy,
                    t!(
                        en: "the plugin used too much memory and was ended",
                        ja: "プラグインがメモリを使いすぎたため終了しました"
                    ),
                    Some(format!(
                        "resident memory {} MiB over the limit of {} MiB",
                        rss >> 20,
                        shared.limits.rss_limit >> 20
                    )),
                );
                return;
            }
        }
        stopped = lock(flag);
    }
}

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn make_side_dir(root: &Path) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let name = format!(
        "konoma-plugin-{}-{}-{nanos}",
        std::process::id(),
        DIR_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let dir = std::path::absolute(root)?.join(name);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

fn start_failure(what: &str, detail: String) -> Failure {
    Failure::external(
        Tr::new(format!("the plugin could not be started: {what}"))
            .ja(format!("プラグインを起動できませんでした: {what}")),
    )
    .with_detail(detail)
}

/// Start a child and shake hands. Returns the connection and the manifest the child declared.
fn connect(
    launch: &dyn Launch,
    cfg: &Config,
    expect_id: Option<&str>,
) -> Result<(Arc<Shared>, Manifest), Failure> {
    let spawned = launch
        .launch()
        .map_err(|e| start_failure("start", e.to_string()))?;
    let side_dir = match make_side_dir(&cfg.scratch_root) {
        Ok(d) => d,
        Err(e) => {
            (spawned.kill)();
            return Err(start_failure("handoff directory", e.to_string()));
        }
    };
    let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_done = Arc::new((Mutex::new(false), Condvar::new()));
    match spawned.stderr {
        Some(mut err) => {
            let tail = Arc::clone(&stderr_tail);
            let done = Arc::clone(&stderr_done);
            let cap = cfg.limits.stderr_tail;
            let _ = std::thread::Builder::new()
                .name("konoma-plugin-stderr".into())
                .spawn(move || {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = err.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        let mut t = lock(&tail);
                        t.extend(&buf[..n]);
                        while t.len() > cap {
                            t.pop_front();
                        }
                    }
                    *lock(&done.0) = true;
                    done.1.notify_all();
                });
        }
        None => *lock(&stderr_done.0) = true,
    }
    let (wtx, wrx) = mpsc::channel::<Vec<u8>>();
    let shared = Arc::new(Shared {
        limits: cfg.limits.clone(),
        side: Side::new(&side_dir, 'h'),
        side_dir: side_dir.clone(),
        state: Mutex::new(State {
            dead: None,
            pending: HashMap::new(),
        }),
        tx: Mutex::new(Some(wtx)),
        kill: spawned.kill,
        pid: spawned.pid,
        stderr_tail,
        stderr_done,
        ending: AtomicBool::new(false),
        next_rid: AtomicU64::new(1),
        live: AtomicUsize::new(0),
        last_used: Mutex::new(Instant::now()),
        stop: (Mutex::new(false), Condvar::new()),
    });

    // The writer: a call never blocks on a child that does not read.
    {
        let shared = Arc::clone(&shared);
        let mut writer = spawned.writer;
        let _ = std::thread::Builder::new()
            .name("konoma-plugin-writer".into())
            .spawn(move || {
                for buf in wrx {
                    if let Err(e) = writer.write_all(&buf).and_then(|()| writer.flush()) {
                        shared.died(&format!("writing to the plugin failed: {e}"));
                        return;
                    }
                }
            });
    }
    // The reader: the handshake answer first, then the answers to calls.
    let (ready_tx, ready_rx) = mpsc::channel::<Result<HelloReply, FrameError>>();
    {
        let shared = Arc::clone(&shared);
        let mut reader = spawned.reader;
        let _ = std::thread::Builder::new()
            .name("konoma-plugin-reader".into())
            .stack_size(8 << 20)
            .spawn(move || {
                match frame::recv::<_, HelloReply>(&mut reader) {
                    Ok(Some(r)) => {
                        // Only an accepted handshake goes on to the call loop: for any other
                        // reply the starter names the reason, and the child's exit that follows
                        // must not get in first.
                        let go = matches!(&r, HelloReply::Ack { version, .. } if host_versions().contains(version));
                        let _ = ready_tx.send(Ok(r));
                        if !go {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = ready_tx.send(Err(FrameError::Truncated));
                        return;
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                }
                read_loop(shared, reader);
            });
    }

    let hello = Hello {
        versions: host_versions(),
        host_version: cfg.host_version.clone(),
        side_dir: path_to_wire(&side_dir),
        lang: cfg.lang.clone(),
    };
    let abort = |what: &str, detail: String| -> Failure {
        let f = shared.fail(
            FailureKind::External,
            Tr::new(format!("the plugin could not be started: {what}"))
                .ja(format!("プラグインを起動できませんでした: {what}")),
            Some(detail),
        );
        f
    };
    let body = frame::encode(&hello).map_err(|e| abort("handshake", e.to_string()))?;
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    let queued = lock(&shared.tx).as_ref().map(|tx| tx.send(buf));
    if !matches!(queued, Some(Ok(()))) {
        return Err(abort("handshake", "the connection is closed".into()));
    }
    let reply = match ready_rx.recv_timeout(cfg.limits.start_timeout) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(abort("handshake", e.to_string())),
        Err(_) => {
            return Err(shared.fail(
                FailureKind::Timeout,
                t!(
                    en: "the plugin did not answer the handshake in time",
                    ja: "プラグインが起動時の応答をしませんでした"
                ),
                Some(format!("no answer within {:?}", cfg.limits.start_timeout)),
            ));
        }
    };
    let manifest = match reply {
        HelloReply::Refused { supported, reason } => {
            return Err(abort(
                "the versions do not match",
                format!("the plugin speaks {supported:?}: {reason}"),
            ));
        }
        HelloReply::Ack { version, manifest } => {
            if !host_versions().contains(&version) {
                return Err(abort(
                    "the versions do not match",
                    format!(
                        "the plugin chose protocol {version}, the host speaks {PROTOCOL_VERSION}"
                    ),
                ));
            }
            match manifest_from_wire(manifest) {
                Ok(m) => m,
                Err(e) => return Err(abort("its manifest", e.to_string())),
            }
        }
    };
    if let Some(id) = expect_id {
        if manifest.id != id {
            return Err(abort(
                "it is another plugin",
                format!("expected {id:?}, the program declared {:?}", manifest.id),
            ));
        }
    }
    if let Some(pid) = shared.pid {
        let s = Arc::clone(&shared);
        let _ = std::thread::Builder::new()
            .name("konoma-plugin-watch".into())
            .spawn(move || watch_loop(s, pid));
    }
    Ok((shared, manifest))
}

// ---------------------------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------------------------

/// A plugin that runs in a child process. See the module docs.
pub struct ProcPlugin {
    manifest: Manifest,
    launch: Box<dyn Launch>,
    cfg: Config,
    /// Checked against the id the program declares when it is started from a cached manifest.
    expect_id: Option<String>,
    conn: Mutex<Option<Arc<Shared>>>,
    next_sid: AtomicU64,
    /// The manifest the program declared at its latest start, when it differs from the cached one.
    learned: Mutex<Option<Manifest>>,
}

impl ProcPlugin {
    /// Start the program now and use the manifest it declares. The child stays up for the first
    /// `open`.
    pub fn start(launch: impl Launch + 'static, cfg: Config) -> Result<ProcPlugin, Failure> {
        let (shared, manifest) = connect(&launch, &cfg, None)?;
        *lock(&shared.last_used) = Instant::now();
        Ok(ProcPlugin {
            expect_id: Some(manifest.id.clone()),
            manifest,
            launch: Box::new(launch),
            cfg,
            conn: Mutex::new(Some(shared)),
            next_sid: AtomicU64::new(1),
            learned: Mutex::new(None),
        })
    }

    /// Build the plugin from a manifest the host cached (VS Code style): **nothing is started** until
    /// the first `open`. The program's declared id must match the cached one, or that `open` fails.
    /// The host saves the cache (see [`super::manifest_to_json`]) and refreshes it from
    /// [`ProcPlugin::learned_manifest`].
    pub fn from_manifest(
        manifest: Manifest,
        launch: impl Launch + 'static,
        cfg: Config,
    ) -> ProcPlugin {
        ProcPlugin {
            expect_id: Some(manifest.id.clone()),
            manifest,
            launch: Box::new(launch),
            cfg,
            conn: Mutex::new(None),
            next_sid: AtomicU64::new(1),
            learned: Mutex::new(None),
        }
    }

    /// The manifest the program declared the last time it was started, if it is not the one this
    /// plugin was built with (the host updates its cache with it).
    pub fn learned_manifest(&self) -> Option<Manifest> {
        lock(&self.learned).clone()
    }

    /// Whether a child is running now.
    pub fn is_running(&self) -> bool {
        lock(&self.conn).as_ref().is_some_and(|c| !c.is_dead())
    }

    /// Stop the child if it has had no live session and no call for `Limits::idle_after`. Returns
    /// whether it was stopped. The host calls this now and then.
    pub fn reap_idle(&self) -> bool {
        self.reap_idle_at(Instant::now())
    }

    /// [`ProcPlugin::reap_idle`] with the time given (so a test does not wait).
    pub fn reap_idle_at(&self, now: Instant) -> bool {
        let mut conn = lock(&self.conn);
        let Some(c) = conn.as_ref() else {
            return false;
        };
        if c.is_dead() {
            *conn = None;
            return false;
        }
        let idle = c.live.load(Ordering::SeqCst) == 0
            && lock(&c.state).pending.is_empty()
            && now.saturating_duration_since(*lock(&c.last_used)) >= self.cfg.limits.idle_after;
        if idle {
            c.fail(
                FailureKind::External,
                t!(en: "the plugin was stopped while idle", ja: "待機中のプラグインを停止しました"),
                None,
            );
            *conn = None;
        }
        idle
    }

    /// End the child now (its sessions fail afterwards).
    pub fn stop(&self) {
        if let Some(c) = lock(&self.conn).take() {
            c.fail(
                FailureKind::External,
                t!(en: "the plugin was stopped", ja: "プラグインを停止しました"),
                None,
            );
        }
    }

    /// The live connection, starting a child if there is none (or it ended). The caller's session
    /// is counted before the lock is released, so `reap_idle` cannot stop the child under it.
    fn connection(&self) -> Result<Arc<Shared>, Failure> {
        let mut conn = lock(&self.conn);
        if let Some(c) = conn.as_ref() {
            if !c.is_dead() {
                c.live.fetch_add(1, Ordering::SeqCst);
                return Ok(Arc::clone(c));
            }
        }
        *conn = None;
        let (shared, manifest) = connect(&*self.launch, &self.cfg, self.expect_id.as_deref())?;
        if manifest_differs(&manifest, &self.manifest) {
            *lock(&self.learned) = Some(manifest);
        }
        shared.live.fetch_add(1, Ordering::SeqCst);
        *conn = Some(Arc::clone(&shared));
        Ok(shared)
    }
}

fn manifest_differs(a: &Manifest, b: &Manifest) -> bool {
    match (super::manifest_to_json(a), super::manifest_to_json(b)) {
        (Ok(x), Ok(y)) => x != y,
        _ => true,
    }
}

impl Drop for ProcPlugin {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Plugin for ProcPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let shared = self.connection()?;
        let sid = self.next_sid.fetch_add(1, Ordering::Relaxed);
        let result = shared.call(
            |rid| {
                let mut enc = Enc::new(&shared.side);
                Ok(ToChild::Open {
                    sid,
                    rid,
                    input: input_to_wire(&input, &mut enc).map_err(Failure::from)?,
                    cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                })
            },
            shared.limits.open_view,
            &cx.cancel,
        );
        match result {
            Ok(Done::Opened(state)) => Ok(Box::new(ProcSession { shared, sid, state })),
            Ok(_) => {
                shared.live.fetch_sub(1, Ordering::SeqCst);
                Err(shared.died("the plugin answered an open with another kind of answer"))
            }
            Err(f) => {
                shared.live.fetch_sub(1, Ordering::SeqCst);
                Err(f)
            }
        }
    }
}

fn cx_wire(
    settings: &crate::Settings,
    lang: &str,
    term: crate::TermCaps,
    external: bool,
    read_scope: &crate::ReadScope,
) -> Result<wire::WCx, Failure> {
    cx_to_wire(&CxSource {
        settings,
        lang,
        term,
        external,
        read_scope,
    })
    .map_err(Failure::from)
}

/// One session of a [`ProcPlugin`].
struct ProcSession {
    shared: Arc<Shared>,
    sid: u64,
    state: SessionState,
}

impl ProcSession {
    fn wrong(&self) -> Failure {
        self.shared
            .died("the plugin answered with another kind of answer than asked for")
    }
}

impl Session for ProcSession {
    fn layout(&self) -> Layout {
        self.state.layout.clone()
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let sid = self.sid;
        let done = self.shared.call(
            |rid| {
                Ok(ToChild::View {
                    sid,
                    rid,
                    req: req_to_wire(req),
                    cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                })
            },
            self.shared.limits.open_view,
            &cx.cancel,
        )?;
        match done {
            Done::Viewed(view, state) => {
                self.state = state;
                Ok(view)
            }
            _ => Err(self.wrong()),
        }
    }

    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let sid = self.sid;
        let done = self.shared.call(
            |rid| {
                Ok(ToChild::Act {
                    sid,
                    rid,
                    action: action.to_string(),
                    cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                })
            },
            self.shared.limits.act_key,
            &cx.cancel,
        )?;
        match done {
            Done::Acted(outcome, state) => {
                self.state = state;
                Ok(outcome)
            }
            _ => Err(self.wrong()),
        }
    }

    fn key(&mut self, key: &Key, cx: &ActCx) -> Result<Outcome, Failure> {
        let sid = self.sid;
        let done = self.shared.call(
            |rid| {
                Ok(ToChild::Key {
                    sid,
                    rid,
                    key: key_to_wire(key),
                    cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                })
            },
            self.shared.limits.act_key,
            &cx.cancel,
        )?;
        match done {
            Done::Acted(outcome, state) => {
                self.state = state;
                Ok(outcome)
            }
            _ => Err(self.wrong()),
        }
    }

    /// An action the child did not report (not declared) counts as enabled, as for a session that
    /// does not override `enabled`.
    fn enabled(&self, action: &str) -> bool {
        self.state.enabled.get(action).copied().unwrap_or(true)
    }
}

impl Drop for ProcSession {
    fn drop(&mut self) {
        let _ = self.shared.send_frame(&ToChild::Close { sid: self.sid });
        self.shared.live.fetch_sub(1, Ordering::SeqCst);
        *lock(&self.shared.last_used) = Instant::now();
    }
}
