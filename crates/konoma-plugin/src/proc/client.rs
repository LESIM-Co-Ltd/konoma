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
//!
//! # Deadlines count the child's work, not its queue
//!
//! The child works through the calls of all its sessions one at a time, in the order they were
//! sent. So a call's clock (its deadline, and the grace after a cancel) runs only while it is the
//! **oldest call in flight**, which is the one the child is working on. A call that waits behind a
//! slow one of another session is not late because of the wait; it starts its clock when the one
//! ahead of it is answered. A call cancelled while it waits is answered "cancelled" by the child
//! when it takes it, without running it.
//!
//! # Stopping
//!
//! `stop`, the idle stop and `Drop` first send `Shutdown` (the child drops its sessions in order
//! and exits) and wait `Limits::shutdown_grace` for it; only a child that has not left by then has
//! its process group killed.

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
    /// After `Shutdown` is sent (a stop, the idle stop, a drop), how long the child has to leave on
    /// its own before its process group is killed. 2 s: dropping the sessions of a plugin is quick;
    /// a plugin that needs longer is hung on something.
    pub shutdown_grace: Duration,
    /// After a start fails, how long the next `open`s fail with that same reason without starting
    /// anything. 10 s: a program that dies at start would otherwise be forked once per open, and a
    /// user holding a key on a file it handles would start it dozens of times a second.
    pub start_retry_after: Duration,
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
            shutdown_grace: Duration::from_secs(2),
            start_retry_after: Duration::from_secs(10),
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

/// Start a program: its own process group, no core dump, and a working directory (`/` unless
/// [`CommandLaunch::with_work_dir`] says otherwise).
pub struct CommandLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The child's working directory.
    pub work_dir: PathBuf,
}

impl CommandLaunch {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        CommandLaunch {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            work_dir: PathBuf::from("/"),
        }
    }

    /// Run the child in `dir` instead of `/` (a host gives it a private directory, so a plugin that
    /// writes to its working directory writes where only the user can read).
    pub fn with_work_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.work_dir = dir.into();
        self
    }
}

impl Launch for CommandLaunch {
    fn launch(&self) -> std::io::Result<Spawned> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(&self.work_dir)
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
    /// The calls in flight, in the order they were sent (the order the child works through them).
    /// The first is the one the child is working on.
    order: VecDeque<u64>,
    /// When the first of `order` became the first (its clock starts there).
    head_since: Option<Instant>,
}

impl State {
    /// A call is over (answered, or given up): it leaves the queue, and the next one starts its
    /// clock now.
    fn leave(&mut self, rid: u64) {
        self.pending.remove(&rid);
        let was_head = self.order.front() == Some(&rid);
        self.order.retain(|r| *r != rid);
        if was_head {
            self.head_since = if self.order.is_empty() {
                None
            } else {
                Some(Instant::now())
            };
        }
    }
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
    /// Why the connection is being stopped on purpose (set before `Shutdown` is sent): the child's
    /// exit that follows is that, not a death.
    planned: Mutex<Option<(FailureKind, Tr, Option<String>)>>,
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
        failure.detail = compose_detail(detail, self.tail_text());
        let pending = {
            let mut st = lock(&self.state);
            if let Some(f) = &st.dead {
                return f.clone();
            }
            st.dead = Some(failure.clone());
            st.order.clear();
            st.head_since = None;
            std::mem::take(&mut st.pending)
        };
        *lock(&self.tx) = None;
        for (_, tx) in pending {
            let _ = tx.send(Err(failure.clone()));
        }
        let _ = std::fs::remove_dir_all(&self.side_dir);
        {
            // Last: whoever waits for the end of the connection sees everything done.
            let (flag, cv) = &self.stop;
            *lock(flag) = true;
            cv.notify_all();
        }
        failure
    }

    fn died(&self, why: &str) -> Failure {
        // A stop on purpose is under way: the child's exit is that, not a death.
        let planned = lock(&self.planned).clone();
        if let Some((kind, message, detail)) = planned {
            return self.fail(kind, message, detail);
        }
        self.fail(
            FailureKind::External,
            t!(
                en: "the plugin stopped working",
                ja: "プラグインが動かなくなりました"
            ),
            Some(why.to_string()),
        )
    }

    /// The bytes of a message on the wire: the length, then the body.
    fn encode_frame(msg: &ToChild) -> Result<Vec<u8>, Failure> {
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
        Ok(buf)
    }

    /// Queue bytes for the writer. False when the connection has ended.
    fn push_frame(&self, buf: Vec<u8>) -> bool {
        matches!(lock(&self.tx).as_ref().map(|tx| tx.send(buf)), Some(Ok(())))
    }

    fn closed(&self) -> Failure {
        self.dead_failure()
            .unwrap_or_else(|| self.died("the connection to the plugin is closed"))
    }

    fn send_frame(&self, msg: &ToChild) -> Result<(), Failure> {
        let buf = Self::encode_frame(msg)?;
        if self.push_frame(buf) {
            Ok(())
        } else {
            Err(self.closed())
        }
    }

    /// Stop the child on purpose: ask it to leave (`Shutdown`: it drops its sessions in order and
    /// exits), give it `Limits::shutdown_grace`, then end the connection, which kills its process
    /// group whatever is left of it.
    fn shutdown(&self, kind: FailureKind, message: Tr, detail: Option<String>) {
        if self.is_dead() {
            return;
        }
        *lock(&self.planned) = Some((kind, message.clone(), detail.clone()));
        if self.send_frame(&ToChild::Shutdown).is_ok() {
            let (flag, cv) = &self.stop;
            let mut stopped = lock(flag);
            let until = Instant::now() + self.limits.shutdown_grace;
            while !*stopped {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                stopped = cv
                    .wait_timeout(stopped, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        self.fail(kind, message, detail);
        // If another thread ended it (the reader saw the child leave), it is done when the flag is.
        let (flag, cv) = &self.stop;
        let mut stopped = lock(flag);
        let until = Instant::now() + STDERR_JOIN * 4;
        while !*stopped {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            stopped = cv
                .wait_timeout(stopped, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Send `build(rid)` and wait for its answer: until `deadline`, and, once `cancel` goes up,
    /// at most `cancel_grace` more. Past either, the child is ended. Both clocks run only while
    /// this call is the oldest in flight, the one the child works on (see the module docs).
    ///
    /// `side` names the side files the message brings; they are deleted if the message never
    /// reaches the child.
    fn call(
        &self,
        build: impl FnOnce(u64) -> Result<(ToChild, Vec<String>), Failure>,
        deadline: Duration,
        cancel: &crate::Cancel,
    ) -> CallResult {
        let rid = self.next_rid.fetch_add(1, Ordering::Relaxed);
        let (msg, side) = build(rid)?;
        let buf = match Self::encode_frame(&msg) {
            Ok(b) => b,
            Err(f) => {
                self.discard_all(&side);
                return Err(f);
            }
        };
        let (tx, rx) = mpsc::channel();
        // The queue order is the send order: both are decided under the state lock.
        let sent = {
            let mut st = lock(&self.state);
            if let Some(f) = &st.dead {
                let f = f.clone();
                drop(st);
                self.discard_all(&side);
                return Err(f);
            }
            st.pending.insert(rid, tx);
            if st.order.is_empty() {
                st.head_since = Some(Instant::now());
            }
            st.order.push_back(rid);
            self.push_frame(buf)
        };
        if !sent {
            lock(&self.state).leave(rid);
            self.discard_all(&side);
            return Err(self.closed());
        }
        let result = self.wait(rid, rx, deadline, cancel);
        lock(&self.state).leave(rid);
        *lock(&self.last_used) = Instant::now();
        result
    }

    fn discard_all(&self, names: &[String]) {
        for n in names {
            self.side.discard(n);
        }
    }

    fn wait(
        &self,
        rid: u64,
        rx: mpsc::Receiver<CallResult>,
        deadline: Duration,
        cancel: &crate::Cancel,
    ) -> CallResult {
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
            // The clock of this call: from when it became the oldest in flight.
            let since = {
                let st = lock(&self.state);
                if st.order.front() == Some(&rid) {
                    Some(st.head_since.unwrap_or(now))
                } else {
                    None
                }
            };
            if let Some(since) = since {
                if now.saturating_duration_since(since) >= deadline {
                    return Err(self.fail(
                        FailureKind::Timeout,
                        t!(
                            en: "the plugin did not answer in time",
                            ja: "プラグインが時間内に応答しませんでした"
                        ),
                        Some(format!("no answer within {deadline:?}")),
                    ));
                }
            }
            if cancel_at.is_none() && cancel.is_cancelled() {
                // A failed send is the connection's end; the loop then ends with its reason.
                let _ = self.send_frame(&ToChild::Cancel { rid });
                cancel_at = Some(now);
            }
            if let (Some(asked), Some(since)) = (cancel_at, since) {
                // The grace is the child's to use once it works on this call.
                if now.saturating_duration_since(asked.max(since)) >= self.limits.cancel_grace {
                    return Err(self.fail(
                        FailureKind::Cancelled,
                        t!(
                            en: "the plugin did not stop when asked and was ended",
                            ja: "中断に応じなかったためプラグインを終了しました"
                        ),
                        Some(format!(
                            "no answer {:?} after the cancel",
                            self.limits.cancel_grace
                        )),
                    ));
                }
            }
        }
    }
}

/// The failure's detail: the reason, then the tail of the child's stderr. Control characters other
/// than newline and tab are dropped (the child wrote them: an escape sequence must not reach the
/// host's terminal), and the whole is at most `MAX_TEXT_CHARS` characters, the end of the stderr
/// kept (the last words are the ones that tell why).
fn compose_detail(reason: Option<String>, stderr: Option<String>) -> Option<String> {
    fn clean(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect()
    }
    let max = crate::text::MAX_TEXT_CHARS;
    let reason = reason.map(|r| clean(&r)).filter(|r| !r.trim().is_empty());
    let stderr = stderr
        .map(|e| clean(e.trim_end()))
        .filter(|e| !e.trim().is_empty());
    let out = match (reason, stderr) {
        (None, None) => return None,
        (Some(r), None) => r,
        (None, Some(e)) => e,
        (Some(r), Some(e)) => {
            let head: String = r.chars().take(max / 3).collect();
            let room = max - head.chars().count() - 1;
            let n = e.chars().count();
            let tail: String = e.chars().skip(n.saturating_sub(room)).collect();
            format!("{head}\n{tail}")
        }
    };
    let n = out.chars().count();
    let out = if n > max {
        out.chars().skip(n - max).collect()
    } else {
        out
    };
    Some(crate::text::clip_text(&out).into_owned())
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
        let tx = {
            let mut st = lock(&shared.state);
            let tx = st.pending.remove(&msg.rid);
            if tx.is_some() {
                // The next call in line is now the one the child works on.
                st.leave(msg.rid);
            }
            tx
        };
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
    let mut dec = Dec::with_total_max(&shared.side, wire::SIDE_TOTAL_MAX_FROM_CHILD);
    match answer {
        Answer::Err(f) => Err(failure_from_wire(f)),
        Answer::Ok(Reply::Opened { state }) => Ok(Done::Opened(state_from_wire(state))),
        Answer::Ok(Reply::Viewed { view, state }) => {
            // Whatever of this view is not read when the conversion fails is deleted.
            let names = wire::view_side_names(&view);
            match view_from_wire(view, &mut dec) {
                Ok(view) => Ok(Done::Viewed(view, state_from_wire(state))),
                Err(e) => {
                    dec.discard(&names);
                    Err(Failure::from(e))
                }
            }
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

/// Remove the handoff directories under `root` that a host which exited abnormally left behind
/// (a normal end removes its own). Only a directory named like the ones [`ProcPlugin`] makes
/// (`konoma-plugin-<pid>-<n>-<nanos>`) is considered, and only when it is a real directory (not a
/// link) owned by this user and the process named in it no longer exists, and is not this process.
/// A pid that has been reused by another process keeps its directory: wrongly keeping a directory
/// is harmless, wrongly removing a live host's is not. Returns how many were removed. The host
/// calls this at start-up.
pub fn sweep_stale_side_dirs(root: &Path) -> usize {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    // SAFETY: plain syscall without arguments.
    let euid = unsafe { libc::geteuid() };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix("konoma-plugin-") else {
            continue;
        };
        let mut parts = rest.split('-');
        let (Some(pid), Some(seq), Some(nanos), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if seq.parse::<u64>().is_err() || nanos.parse::<u128>().is_err() {
            continue;
        }
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        let path = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || meta.uid() != euid {
            continue;
        }
        // SAFETY: signal 0 only asks whether the process exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Remove the per-process private directories `konoma-<kind>-<pid>` (the host's `private_dir(kind)`,
/// the parent of the handoff directories) under `temp` that a host which exited abnormally left
/// behind. [`sweep_stale_side_dirs`] only looks inside one such directory, and a host only sees its
/// own, so what a crashed host left is found here. The same safety as there: a real directory owned
/// by this user, named exactly like that (the pid, optionally followed by the 16 hex digits of the
/// fallback name), whose process no longer exists and is not this one. Returns how many were removed.
pub fn sweep_stale_private_roots(temp: &Path, kind: &str) -> usize {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(temp) else {
        return 0;
    };
    let prefix = format!("konoma-{kind}-");
    // SAFETY: plain syscall without arguments.
    let euid = unsafe { libc::geteuid() };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let mut parts = rest.split('-');
        let (Some(pid), random, None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if random.is_some_and(|r| r.len() != 16 || !r.bytes().all(|b| b.is_ascii_hexdigit())) {
            continue;
        }
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        let path = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || meta.uid() != euid {
            continue;
        }
        // SAFETY: signal 0 only asks whether the process exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
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
    let fail_start = |what: &str, detail: String| -> Failure {
        (spawned.kill)();
        let _ = std::fs::remove_dir_all(&side_dir);
        start_failure(what, detail)
    };
    let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_done = Arc::new((Mutex::new(false), Condvar::new()));
    match spawned.stderr {
        Some(mut err) => {
            let tail = Arc::clone(&stderr_tail);
            let done = Arc::clone(&stderr_done);
            let cap = cfg.limits.stderr_tail;
            let spawned_thread = std::thread::Builder::new()
                .name("konoma-plugin-stderr".into())
                .spawn(move || {
                    let mut buf = [0u8; 4096];
                    loop {
                        match err.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let mut t = lock(&tail);
                                t.extend(&buf[..n]);
                                while t.len() > cap {
                                    t.pop_front();
                                }
                            }
                            // A signal is not the end of the stream.
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                    *lock(&done.0) = true;
                    done.1.notify_all();
                });
            if let Err(e) = spawned_thread {
                return Err(fail_start("its stderr reader", e.to_string()));
            }
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
            order: VecDeque::new(),
            head_since: None,
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
        planned: Mutex::new(None),
    });
    let abort = |what: &str, detail: String| -> Failure {
        shared.fail(
            FailureKind::External,
            Tr::new(format!("the plugin could not be started: {what}"))
                .ja(format!("プラグインを起動できませんでした: {what}")),
            Some(detail),
        )
    };

    // The writer: a call never blocks on a child that does not read.
    {
        let shared = Arc::clone(&shared);
        let mut writer = spawned.writer;
        let r = std::thread::Builder::new()
            .name("konoma-plugin-writer".into())
            .spawn(move || {
                for buf in wrx {
                    if let Err(e) = writer.write_all(&buf).and_then(|()| writer.flush()) {
                        shared.died(&format!("writing to the plugin failed: {e}"));
                        return;
                    }
                }
            });
        if let Err(e) = r {
            return Err(abort("its writer thread", e.to_string()));
        }
    }
    // The reader: the handshake answer first, then the answers to calls.
    let (ready_tx, ready_rx) = mpsc::channel::<Result<HelloReply, FrameError>>();
    {
        let shared = Arc::clone(&shared);
        let mut reader = spawned.reader;
        let r = std::thread::Builder::new()
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
        if let Err(e) = r {
            return Err(abort("its reader thread", e.to_string()));
        }
    }

    let hello = Hello {
        versions: host_versions(),
        host_version: cfg.host_version.clone(),
        side_dir: path_to_wire(&side_dir),
        lang: cfg.lang.clone(),
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
        let r = std::thread::Builder::new()
            .name("konoma-plugin-watch".into())
            .spawn(move || watch_loop(s, pid));
        if let Err(e) = r {
            // Without the memory watch the limit is not kept: not a start worth having.
            return Err(abort("its memory watch", e.to_string()));
        }
    }
    Ok((shared, manifest))
}

// ---------------------------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------------------------

/// What a [`ProcPlugin`] holds of its child. A start happens with the lock released, so `stop`,
/// `is_running` and `reap_idle` never wait for it.
enum Slot {
    Idle,
    /// A thread is starting the child; the other opens wait for it.
    Starting,
    Live(Arc<Shared>),
    /// The latest start failed: until `until`, opens fail with this reason without starting again.
    Failed {
        until: Instant,
        failure: Failure,
    },
}

/// A plugin that runs in a child process. See the module docs.
pub struct ProcPlugin {
    manifest: Manifest,
    launch: Box<dyn Launch>,
    cfg: Config,
    /// Checked against the id the program declares when it is started from a cached manifest.
    expect_id: Option<String>,
    conn: Mutex<Slot>,
    /// Woken when a start ends (the opens that wait for it look again).
    conn_cv: Condvar,
    /// Counts the `stop`s: a start that finds it changed was stopped meanwhile.
    stop_gen: AtomicU64,
    next_sid: AtomicU64,
    /// The manifest the program declared at its latest start, when it differs from the cached one.
    learned: Mutex<Option<Manifest>>,
}

impl ProcPlugin {
    /// Start the program now and use the manifest it declares. The child stays up for the first
    /// `open`.
    pub fn start(launch: impl Launch + 'static, cfg: Config) -> Result<ProcPlugin, Failure> {
        let first = cfg.limits.start_timeout;
        ProcPlugin::start_within(launch, cfg, first)
    }

    /// [`ProcPlugin::start`] with a different bound for this first handshake only. The plugin keeps
    /// `cfg` as given, so a later restart is bound by `Limits::start_timeout` like one of a plugin
    /// built by [`ProcPlugin::from_manifest`].
    pub fn start_within(
        launch: impl Launch + 'static,
        cfg: Config,
        first_handshake: Duration,
    ) -> Result<ProcPlugin, Failure> {
        let mut first_cfg = cfg.clone();
        first_cfg.limits.start_timeout = first_handshake;
        let (shared, manifest) = connect(&launch, &first_cfg, None)?;
        *lock(&shared.last_used) = Instant::now();
        Ok(ProcPlugin {
            expect_id: Some(manifest.id.clone()),
            manifest,
            launch: Box::new(launch),
            cfg,
            conn: Mutex::new(Slot::Live(shared)),
            conn_cv: Condvar::new(),
            stop_gen: AtomicU64::new(0),
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
            conn: Mutex::new(Slot::Idle),
            conn_cv: Condvar::new(),
            stop_gen: AtomicU64::new(0),
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
        matches!(&*lock(&self.conn), Slot::Live(c) if !c.is_dead())
    }

    /// Stop the child if it has had no live session and no call for `Limits::idle_after`. Returns
    /// whether it was stopped. The host calls this now and then.
    pub fn reap_idle(&self) -> bool {
        self.reap_idle_at(Instant::now())
    }

    /// [`ProcPlugin::reap_idle`] with the time given (so a test does not wait).
    pub fn reap_idle_at(&self, now: Instant) -> bool {
        let taken = {
            let mut conn = lock(&self.conn);
            let Slot::Live(c) = &*conn else {
                return false;
            };
            if c.is_dead() {
                *conn = Slot::Idle;
                return false;
            }
            let idle = c.live.load(Ordering::SeqCst) == 0
                && lock(&c.state).pending.is_empty()
                && now.saturating_duration_since(*lock(&c.last_used)) >= self.cfg.limits.idle_after;
            if !idle {
                return false;
            }
            match std::mem::replace(&mut *conn, Slot::Idle) {
                Slot::Live(c) => c,
                _ => return false,
            }
        };
        // Outside the lock: the child is given its grace to leave.
        taken.shutdown(
            FailureKind::External,
            t!(en: "the plugin was stopped while idle", ja: "待機中のプラグインを停止しました"),
            None,
        );
        true
    }

    /// End the child now (its sessions fail afterwards). The child is asked to leave first and
    /// killed if it has not within `Limits::shutdown_grace`. A start that is under way when this is
    /// called ends in a stopped child.
    pub fn stop(&self) {
        self.stop_gen.fetch_add(1, Ordering::SeqCst);
        let taken = {
            let mut conn = lock(&self.conn);
            match &*conn {
                // The starter sees the changed count and stops what it started.
                Slot::Starting => None,
                _ => match std::mem::replace(&mut *conn, Slot::Idle) {
                    Slot::Live(c) => Some(c),
                    _ => None,
                },
            }
        };
        if let Some(c) = taken {
            c.shutdown(
                FailureKind::External,
                t!(en: "the plugin was stopped", ja: "プラグインを停止しました"),
                None,
            );
        }
    }

    /// The live connection, starting a child if there is none (or it ended). The caller's session
    /// is counted before the lock is released, so `reap_idle` cannot stop the child under it.
    fn connection(&self) -> Result<Arc<Shared>, Failure> {
        let gen = {
            let mut conn = lock(&self.conn);
            loop {
                match &*conn {
                    Slot::Live(c) if !c.is_dead() => {
                        c.live.fetch_add(1, Ordering::SeqCst);
                        return Ok(Arc::clone(c));
                    }
                    Slot::Failed { until, failure } if Instant::now() < *until => {
                        return Err(failure.clone());
                    }
                    Slot::Starting => {
                        conn = self.conn_cv.wait(conn).unwrap_or_else(|e| e.into_inner());
                    }
                    _ => {
                        *conn = Slot::Starting;
                        break self.stop_gen.load(Ordering::SeqCst);
                    }
                }
            }
        };
        let started = connect(&*self.launch, &self.cfg, self.expect_id.as_deref());
        let mut conn = lock(&self.conn);
        let result = match started {
            Ok((shared, manifest)) => {
                if self.stop_gen.load(Ordering::SeqCst) != gen {
                    *conn = Slot::Idle;
                    drop(conn);
                    self.conn_cv.notify_all();
                    shared.shutdown(
                        FailureKind::External,
                        t!(en: "the plugin was stopped", ja: "プラグインを停止しました"),
                        None,
                    );
                    return Err(Failure::external(t!(
                        en: "the plugin was stopped while it started",
                        ja: "起動中にプラグインが停止されました"
                    )));
                }
                if manifest_differs(&manifest, &self.manifest) {
                    *lock(&self.learned) = Some(manifest);
                }
                shared.live.fetch_add(1, Ordering::SeqCst);
                *conn = Slot::Live(Arc::clone(&shared));
                Ok(shared)
            }
            Err(f) => {
                *conn = Slot::Failed {
                    until: Instant::now() + self.cfg.limits.start_retry_after,
                    failure: f.clone(),
                };
                Err(f)
            }
        };
        drop(conn);
        self.conn_cv.notify_all();
        result
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
                let built = input_to_wire(&input, &mut enc)
                    .map_err(Failure::from)
                    .and_then(|input| {
                        let cx =
                            cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?;
                        Ok((input, cx))
                    });
                match built {
                    Ok((input, cx)) => {
                        let side = wire::input_side_names(&input);
                        Ok((
                            ToChild::Open {
                                sid,
                                rid,
                                input,
                                cx,
                            },
                            side,
                        ))
                    }
                    Err(f) => {
                        // Files already written for this message are not going to be read.
                        enc.discard();
                        Err(f)
                    }
                }
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
                Ok((
                    ToChild::View {
                        sid,
                        rid,
                        req: req_to_wire(req),
                        cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                    },
                    Vec::new(),
                ))
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
                Ok((
                    ToChild::Act {
                        sid,
                        rid,
                        action: action.to_string(),
                        cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                    },
                    Vec::new(),
                ))
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
                Ok((
                    ToChild::Key {
                        sid,
                        rid,
                        key: key_to_wire(key),
                        cx: cx_wire(cx.settings, cx.lang, cx.term, cx.external, &cx.read_scope)?,
                    },
                    Vec::new(),
                ))
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
