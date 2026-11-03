//! A deliberately raw peer: unlike ProcPlugin it must observe refusal, natural exit and bad input.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use konoma_plugin::proc::{frame, proto::*, wire};
use konoma_plugin::{Input, Manifest, ViewReq};

use crate::{Limits, Options};

type WriteJob = (Vec<u8>, mpsc::SyncSender<Result<(), String>>);

static SIGNAL_CLEANUP: AtomicBool = AtomicBool::new(false);
static ACTIVE_GROUP: AtomicI32 = AtomicI32::new(0);

extern "C" fn stop_on_signal(signal: libc::c_int) {
    let pid = ACTIVE_GROUP.load(Ordering::SeqCst);
    // SAFETY: only async-signal-safe syscalls; the published pid names the CLI's owned group.
    unsafe {
        if pid > 0 {
            libc::kill(-pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        libc::_exit(128 + signal);
    }
}

/// Installed only by the executable, leaving embedding applications' signal policy alone.
pub(crate) fn install_signal_cleanup() -> std::io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: zeroed sigaction is initialized before installation; the handler is static.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = stop_on_signal as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    SIGNAL_CLEANUP.store(true, Ordering::SeqCst);
    Ok(())
}

struct BlockSignals(Option<libc::sigset_t>);

impl BlockSignals {
    fn new() -> std::io::Result<Self> {
        if !SIGNAL_CLEANUP.load(Ordering::SeqCst) {
            return Ok(Self(None));
        }
        // The CLI has joined the preceding connection's threads before spawning another child.
        // Block delivery until the new group is published, closing the spawn/registration gap.
        unsafe {
            let mut set = std::mem::zeroed();
            let mut old = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGTERM);
            let error = libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
            if error != 0 {
                return Err(std::io::Error::from_raw_os_error(error));
            }
            Ok(Self(Some(old)))
        }
    }
}

impl Drop for BlockSignals {
    fn drop(&mut self) {
        if let Some(old) = &self.0 {
            // SAFETY: restore this thread's saved mask after the group was published.
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, old, std::ptr::null_mut());
            }
        }
    }
}

pub(crate) struct Connection {
    pub side: wire::Side,
    pid: i32,
    writer: Option<mpsc::SyncSender<WriteJob>>,
    frames: mpsc::Receiver<Result<Vec<u8>, String>>,
    exit: mpsc::Receiver<std::io::Result<ExitStatus>>,
    tail: Arc<Mutex<VecDeque<u8>>>,
    threads: Vec<JoinHandle<()>>,
    stopped: bool,
}

impl Connection {
    pub fn spawn(options: &Options, limits: &Limits) -> Result<Self, String> {
        let blocked = BlockSignals::new().map_err(|e| e.to_string())?;
        let child_mask = blocked.0;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "konoma-plugin-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| e.to_string())?;
        // Inherit the caller's directory: an interpreter argument such as `plugin.py` must keep
        // its meaning. Only the handoff directory is private; arbitrary arguments stay intact.
        let mut cmd = Command::new(&options.program);
        cmd.args(&options.args);
        if let Some(id) = &options.plugin {
            cmd.args(["--plugin", id]);
        }
        cmd.arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: setrlimit is async-signal-safe between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if let Some(mask) = &child_mask {
                    if libc::sigprocmask(libc::SIG_SETMASK, mask, std::ptr::null_mut()) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                let none = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::setrlimit(libc::RLIMIT_CORE, &none) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e.to_string());
            }
        };
        let pid = child.id() as i32;
        if SIGNAL_CLEANUP.load(Ordering::SeqCst) {
            ACTIVE_GROUP.store(pid, Ordering::SeqCst);
        }
        drop(blocked);
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let (exit_tx, exit) = mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || {
            let _ = exit_tx.send(child.wait());
        });
        let (frame_tx, frames) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || loop {
            let result = match frame::read_frame(&mut stdout) {
                Ok(Some(body)) => Ok(body),
                Ok(None) => Err("EOF (the child closed stdout)".into()),
                Err(e) => Err(e.to_string()),
            };
            let ended = result.is_err();
            if frame_tx.send(result).is_err() || ended {
                break;
            }
        });
        let (writer, jobs) = mpsc::sync_channel::<WriteJob>(1);
        let sender = std::thread::spawn(move || {
            while let Ok((bytes, done)) = jobs.recv() {
                let result = stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.flush())
                    .map_err(|e| e.to_string());
                let ended = result.is_err();
                let _ = done.send(result);
                if ended {
                    break;
                }
            }
        });
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&tail);
        let max_tail = limits.stderr_tail;
        let logger = std::thread::spawn(move || {
            let mut buf = [0; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut tail = sink.lock().unwrap_or_else(|e| e.into_inner());
                        tail.extend(&buf[..n]);
                        let excess = tail.len().saturating_sub(max_tail);
                        tail.drain(..excess);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            side: wire::Side::new(dir, 'h'),
            pid,
            writer: Some(writer),
            frames,
            exit,
            tail,
            threads: vec![waiter, reader, sender, logger],
            stopped: false,
        })
    }

    pub fn packet(body: Vec<u8>) -> Vec<u8> {
        let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
        bytes.extend(body);
        bytes
    }

    pub fn write(&self, bytes: Vec<u8>, until: Instant) -> Result<(), String> {
        let (done, result) = mpsc::sync_channel(1);
        // A connection never queues more than one write. The worker keeps a blocked pipe off
        // this thread, and killing the process group releases it on every error path.
        self.writer
            .as_ref()
            .ok_or("closed input")?
            .send((bytes, done))
            .map_err(|_| "closed input")?;
        result
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|_| "deadline exceeded while writing".to_string())?
    }

    fn receive(&self, until: Instant) -> Result<Vec<u8>, String> {
        self.frames
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|_| "deadline exceeded waiting for a frame".to_string())?
    }

    pub fn hello(&self, versions: Vec<u32>, timeout: Duration) -> Result<HelloReply, String> {
        let until = Instant::now() + timeout;
        let hello = Hello {
            versions,
            host_version: konoma_plugin::proc::client::Config::default().host_version,
            side_dir: wire::path_to_wire(self.side.dir()),
            lang: "en".into(),
        };
        self.write(
            Self::packet(frame::encode(&hello).map_err(|e| e.to_string())?),
            until,
        )?;
        frame::decode(&self.receive(until)?).map_err(|e| e.to_string())
    }

    pub fn handshake(&self, limits: &Limits, selected: Option<&str>) -> Result<Manifest, String> {
        let versions = host_versions();
        match self.hello(versions.clone(), limits.start_timeout)? {
            HelloReply::Ack { version, manifest } => {
                if !versions.contains(&version) {
                    return Err(format!("Ack version {version}, offered {versions:?}"));
                }
                let manifest = wire::manifest_from_wire(manifest)
                    .map_err(|e| format!("unreadable manifest: {e}"))?;
                if selected.is_some_and(|id| id != manifest.id) {
                    return Err(format!(
                        "manifest id {:?}, selected {selected:?}",
                        manifest.id
                    ));
                }
                Ok(manifest)
            }
            other => Err(format!("{other:?}")),
        }
    }

    pub fn send(&self, msg: &ToChild, until: Instant) -> Result<(), String> {
        self.write(
            Self::packet(frame::encode(msg).map_err(|e| e.to_string())?),
            until,
        )
    }

    pub fn answer(&self, rid: u64, until: Instant) -> Result<Answer, String> {
        let answer: FromChild = frame::decode(&self.receive(until)?).map_err(|e| e.to_string())?;
        if answer.rid != rid {
            return Err(format!("answer rid {}, requested {rid}", answer.rid));
        }
        Ok(answer.answer)
    }

    pub fn call(&self, msg: &ToChild, rid: u64, timeout: Duration) -> Result<Answer, String> {
        let until = Instant::now() + timeout;
        self.send(msg, until)?;
        self.answer(rid, until)
    }

    pub fn open(&self, file: &std::path::Path, limits: &Limits) -> Result<WState, String> {
        let file = std::path::absolute(file).map_err(|e| e.to_string())?;
        let input = wire::input_to_wire(
            &Input::File {
                path: file,
                hint: None,
            },
            &mut wire::Enc::new(&self.side),
        )
        .map_err(|e| e.to_string())?;
        match self.call(
            &ToChild::Open {
                sid: 1,
                rid: 1,
                input,
                cx: cx(),
            },
            1,
            limits.open_view,
        )? {
            Answer::Ok(Reply::Opened { state }) => Ok(state),
            other => Err(format!("{other:?}")),
        }
    }

    pub fn view(sid: u64, rid: u64) -> ToChild {
        ToChild::View {
            sid,
            rid,
            req: wire::req_to_wire(&ViewReq::new(80, 24)),
            cx: cx(),
        }
    }

    /// A second reply proves the child can still service requests after an Err/act/key.
    pub fn alive(&self, limits: &Limits) -> Result<(), String> {
        match self.call(&Self::view(u64::MAX, 100), 100, limits.open_view)? {
            Answer::Err(_) => Ok(()),
            other => Err(format!("{other:?} for unopened sid")),
        }
    }

    pub fn exited(&self, timeout: Duration) -> Result<ExitStatus, String> {
        self.exit
            .recv_timeout(timeout)
            .map_err(|_| "deadline exceeded waiting for child exit".to_string())?
            .map_err(|e| e.to_string())
    }

    pub fn exit_if_any(&self, timeout: Duration) -> Result<Option<ExitStatus>, String> {
        match self.exit.recv_timeout(timeout) {
            Ok(result) => result.map(Some).map_err(|e| e.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("lost process exit status".into()),
        }
    }

    pub fn finish(&mut self) -> String {
        if !self.stopped {
            self.stopped = true;
            // SAFETY: pid is the group leader created by process_group(0). Never a name match.
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
            self.writer.take();
            // Release a reader that may be waiting to hand off unsolicited frames.
            let (_tx, empty) = mpsc::channel();
            self.frames = empty;
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
            let _ = ACTIVE_GROUP.compare_exchange(self.pid, 0, Ordering::SeqCst, Ordering::SeqCst);
            let _ = std::fs::remove_dir_all(self.side.dir());
        }
        let bytes: Vec<u8> = self
            .tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.finish();
    }
}

pub(crate) fn cx() -> wire::WCx {
    wire::WCx {
        settings: Vec::new(),
        lang: "en".into(),
        pixels: false,
        colors: 0,
        external: false,
        read_dirs: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn blocked() -> Connection {
        let options = Options {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exec /bin/cat".into()],
            plugin: None,
            files: vec![],
        };
        Connection::spawn(&options, &Limits::default()).unwrap()
    }

    fn assert_request_deadline(
        check: impl FnOnce(&Connection, &Limits) -> Result<(), String> + Send + 'static,
    ) {
        let options = Options {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "while IFS= read -r request; do :; done".into()],
            plugin: None,
            files: vec![],
        };
        let limits = Limits {
            open_view: Duration::from_millis(250),
            act_key: Duration::from_millis(250),
            cancel_grace: Duration::from_millis(250),
            ..Limits::default()
        };
        let connection = Connection::spawn(&options, &limits).unwrap();
        let pid = connection.pid;
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = check(&connection, &limits);
            drop(connection);
            let _ = tx.send(result);
        });
        // This outer bound is a cleanup watchdog, independent of the deadline being tested.
        // If a mutation removes recv_timeout, kill this exact group to release the blocked read,
        // then join the worker before failing. No mutated test can leave its child running.
        let result = rx.recv_timeout(Limits::default().shutdown_grace);
        if result.is_err() {
            // SAFETY: pid belongs to the connection's own process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        worker.join().unwrap();
        let error = result
            .expect("request deadline was not enforced; watchdog cleaned up the child")
            .expect_err("a child that never answers must fail");
        assert!(
            error.contains("deadline"),
            "expected deadline failure, got {error}"
        );
    }

    #[test]
    fn open_deadline_is_enforced() {
        assert_request_deadline(|c, limits| {
            c.open(
                std::path::Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/input.konoma-demo"
                )),
                limits,
            )
            .map(|_| ())
        });
    }

    #[test]
    fn view_deadline_is_enforced() {
        assert_request_deadline(|c, limits| {
            c.call(&Connection::view(1, 2), 2, limits.open_view)
                .map(|_| ())
        });
    }

    #[test]
    fn action_deadline_is_enforced() {
        assert_request_deadline(|c, limits| {
            c.call(
                &ToChild::Act {
                    sid: 1,
                    rid: 4,
                    action: "change".into(),
                    cx: cx(),
                },
                4,
                limits.act_key,
            )
            .map(|_| ())
        });
    }

    #[test]
    fn cancelled_call_deadline_is_enforced() {
        assert_request_deadline(|c, limits| {
            let until = Instant::now() + limits.cancel_grace;
            c.send(&Connection::view(1, 3), until)?;
            c.send(&ToChild::Cancel { rid: 3 }, until)?;
            c.answer(3, until).map(|_| ())
        });
    }

    #[test]
    fn drop_kills_and_reaps_its_process_group() {
        let c = blocked();
        let pid = c.pid;
        drop(c);
        // SAFETY: signal 0 only queries the specific process group, without sending a signal.
        assert_eq!(unsafe { libc::kill(-pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn cleanup_kills_other_group_members_too() {
        let c = blocked();
        // Join a second process to the owned group. Keeping it as our child lets this test wait
        // for its death without ps, polling, zombie races or relying on the OS orphan reaper.
        let mut member = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(c.pid)
            .spawn()
            .unwrap();
        let member_pid = member.id() as i32;
        // Child::wait closes its stdin handle. Hold it here so EOF cannot let cat exit before
        // the group kill; the assertion must not depend on which thread gets scheduled first.
        let member_input = member.stdin.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || {
            let _ = tx.send(member.wait());
        });
        drop(c);
        let status = rx.recv_timeout(Limits::default().shutdown_grace);
        if status.is_err() {
            // On failure still clean up this exact owned process before reporting it.
            unsafe {
                libc::kill(member_pid, libc::SIGKILL);
            }
        }
        waiter.join().unwrap();
        drop(member_input);
        assert_eq!(status.unwrap().unwrap().signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn unwinding_also_reaps_the_child() {
        let c = blocked();
        let pid = c.pid;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _connection = c;
            panic!("exercise the guard");
        }));
        assert!(result.is_err());
        // SAFETY: query the specific group that the guard owned.
        assert_eq!(unsafe { libc::kill(-pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn stderr_is_drained_and_bounded() {
        let options = Options {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 123456789tail >&2; exec /bin/cat".into(),
            ],
            plugin: None,
            files: vec![],
        };
        let mut c = Connection::spawn(
            &options,
            &Limits {
                stderr_tail: 4,
                ..Limits::default()
            },
        )
        .unwrap();
        // The echoed frame is a readiness barrier after the shell wrote stderr.
        let until = Instant::now() + Limits::default().start_timeout;
        c.write(Connection::packet(vec![1]), until).unwrap();
        assert_eq!(c.receive(until).unwrap(), vec![1]);
        assert_eq!(c.finish(), "tail");
    }
}
