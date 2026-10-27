//! The child side: answer the host's calls over a pair of byte streams.
//!
//! One thread reads (so a `Cancel` is seen while a call runs), one thread works through the calls
//! in order. A panic in the plugin is caught and answered as a failure; the process goes on.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::frame::{self, FrameError, MAX_FRAME};
use super::proto::{
    Answer, FromChild, Hello, HelloReply, Reply, ToChild, WState, PROTOCOL_VERSION,
};
use super::wire::{
    self, cx_from_wire, failure_to_wire, input_from_wire, input_side_names, key_from_wire,
    layout_to_wire, manifest_to_wire, outcome_to_wire, req_from_wire, view_to_wire, CxData, Dec,
    Enc, Side, WView, SIDE_TOTAL_MAX_FROM_CHILD,
};
use crate::session::{ActCx, Cancel, OpenCx, ViewCx};
use crate::{t, Failure, Plugin, Session};

/// Knobs of [`serve_io`]; the defaults are what [`serve`] uses.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The most sessions the child keeps open at once; an `Open` over it fails with `TooHeavy`.
    /// (The host closes sessions it is done with; this bounds a host that does not.)
    pub max_sessions: usize,
    /// The size over which a byte string goes through a side file; `None` is the protocol's.
    pub side_threshold: Option<usize>,
    /// When the host's end of the pipe closes (the host died, or left without `Shutdown`), every
    /// running call is asked to stop and, after this long, the **process exits**. `None` leaves
    /// the process alone (a test serves from a thread). [`serve`] sets `HOST_GONE_GRACE`.
    pub exit_after_host_gone: Option<Duration>,
}

impl Default for ServeOptions {
    fn default() -> ServeOptions {
        ServeOptions {
            max_sessions: 1024,
            side_threshold: None,
            exit_after_host_gone: None,
        }
    }
}

/// How long a child that has lost its host gets to finish what it is doing (its calls are asked to
/// stop at once) before the process exits. 2 s, the same as the host's grace for a cancel.
pub const HOST_GONE_GRACE: Duration = Duration::from_secs(2);

enum Job {
    Call {
        msg: ToChild,
        flag: Arc<AtomicBool>,
    },
    Notice(ToChild),
    /// The stream ended; the text is why, when it did not end cleanly.
    End(Option<String>),
}

/// Serve the host over this process's standard input and output.
///
/// The protocol must own the original stdout, and nothing else may write to it: a stray `println!`
/// in the plugin (or in a library it uses) would corrupt a frame. So the original stdout is
/// duplicated for the protocol and fd 1 is pointed at stderr (which the host keeps the tail of).
/// Stdin is duplicated the same way and fd 0 pointed at `/dev/null`, so the plugin cannot read the
/// protocol either.
pub fn serve<P: Plugin>(plugin: P) -> std::io::Result<()> {
    let (input, output) = take_protocol_fds()?;
    serve_io(
        plugin,
        input,
        output,
        &ServeOptions {
            exit_after_host_gone: Some(HOST_GONE_GRACE),
            ..ServeOptions::default()
        },
    )
}

/// Duplicate fds 0 and 1 for the protocol (close-on-exec, so a program the plugin starts does not
/// inherit them: a grandchild that held the pipe would keep the host from ever seeing the end of
/// it), then point fd 1 at stderr and fd 0 at `/dev/null`.
fn take_protocol_fds() -> std::io::Result<(std::fs::File, std::fs::File)> {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    // `F_DUPFD_CLOEXEC` from 3 up, so the copy cannot land on 0, 1 or 2.
    let dup_cloexec = |fd: libc::c_int| -> std::io::Result<File> {
        // SAFETY: plain fd duplication on this process's own descriptor; the new fd is wrapped in
        // exactly one `File` that owns it (so it is closed on every early return).
        let new = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if new < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `new` is a fresh descriptor nothing else owns.
        Ok(unsafe { File::from_raw_fd(new) })
    };
    // Each copy is owned by a `File` from the moment it exists, so a failure of the second or of
    // the redirection below closes the first instead of leaking it.
    let output = dup_cloexec(1)?;
    let input = dup_cloexec(0)?;
    // SAFETY: plain syscalls on this process's own descriptors.
    unsafe {
        if libc::dup2(2, 1) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if null >= 0 {
            libc::dup2(null, 0);
            if null > 2 {
                libc::close(null);
            }
        }
    }
    Ok((input, output))
}

/// Serve over any pair of streams (a test serves over pipes from a thread). Returns when the host
/// closes the stream or sends `Shutdown`; an error is a broken handshake or a broken stream.
pub fn serve_io<P, R, W>(
    plugin: P,
    mut input: R,
    mut output: W,
    opts: &ServeOptions,
) -> std::io::Result<()>
where
    P: Plugin,
    R: Read + Send + 'static,
    W: Write,
{
    let io_err = |e: FrameError| std::io::Error::other(e.to_string());
    let hello: Hello = match frame::recv(&mut input).map_err(io_err)? {
        Some(h) => h,
        None => return Ok(()),
    };
    if !hello.versions.contains(&PROTOCOL_VERSION) {
        let reply = HelloReply::Refused {
            supported: vec![PROTOCOL_VERSION],
            reason: format!(
                "the host speaks protocol versions {:?}, this plugin speaks {PROTOCOL_VERSION}",
                hello.versions
            ),
        };
        frame::send(&mut output, &reply).map_err(io_err)?;
        return Ok(());
    }
    let manifest = catch_unwind(AssertUnwindSafe(|| plugin.manifest()))
        .map_err(|_| std::io::Error::other("the plugin panicked in manifest()"))?;
    let action_ids: Vec<String> = manifest.actions.iter().map(|a| a.id.clone()).collect();
    let wm = manifest_to_wire(&manifest).map_err(|e| std::io::Error::other(e.to_string()))?;
    frame::send(
        &mut output,
        &HelloReply::Ack {
            version: PROTOCOL_VERSION,
            manifest: wm,
        },
    )
    .map_err(io_err)?;

    let dir = wire::abs_path_from_wire(hello.side_dir, "handoff directory")
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut side = Side::new(dir, 'c');
    if let Some(t) = opts.side_threshold {
        side = side.with_threshold(t);
    }

    // The reader: raises a call's cancel flag the moment `Cancel` arrives, and queues the rest.
    let flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> = Arc::default();
    let (tx, rx) = mpsc::channel::<Job>();
    let reader_flags = Arc::clone(&flags);
    let exit_after = opts.exit_after_host_gone;
    let reader = std::thread::Builder::new()
        .name("konoma-plugin-reader".into())
        .stack_size(8 << 20)
        .spawn(move || loop {
            let job = match frame::recv::<_, ToChild>(&mut input) {
                Ok(None) => Job::End(None),
                Err(e) => Job::End(Some(e.to_string())),
                Ok(Some(ToChild::Cancel { rid })) => {
                    if let Some(f) = lock(&reader_flags).get(&rid) {
                        f.store(true, Ordering::SeqCst);
                    }
                    continue;
                }
                Ok(Some(ToChild::Shutdown)) => {
                    // Stop what runs and what waits: the calls ahead of the `Shutdown` in the
                    // queue answer "cancelled" when they are taken, and the process can leave.
                    raise_all(&reader_flags);
                    Job::Notice(ToChild::Shutdown)
                }
                Ok(Some(msg)) => match call_rid(&msg) {
                    Some(rid) => {
                        let flag = Arc::new(AtomicBool::new(false));
                        lock(&reader_flags).insert(rid, Arc::clone(&flag));
                        Job::Call { msg, flag }
                    }
                    None => Job::Notice(msg),
                },
            };
            let end = matches!(job, Job::End(_));
            if end {
                // The host is gone: nobody is waiting for any running call.
                raise_all(&reader_flags);
            }
            let queued = tx.send(job).is_ok();
            if end {
                if let Some(grace) = exit_after {
                    // The worker may be inside a plugin call that never looks at its flag. The
                    // process is the plugin's to give up, not the OS's to find: leave after the
                    // grace even if the worker has not (a clean exit gets there first).
                    std::thread::sleep(grace);
                    std::process::exit(1);
                }
                return;
            }
            if !queued {
                return;
            }
        })?;

    let mut worker = Worker {
        plugin: Arc::new(plugin),
        action_ids,
        sessions: HashMap::new(),
        side,
        max_sessions: opts.max_sessions,
        written: Vec::new(),
    };
    let mut result = Ok(());
    for job in rx {
        match job {
            Job::End(None) => break,
            Job::End(Some(why)) => {
                result = Err(std::io::Error::other(why));
                break;
            }
            Job::Notice(ToChild::Shutdown) => break,
            Job::Notice(ToChild::Close { sid }) => {
                if let Some(session) = worker.sessions.remove(&sid) {
                    quiet_drop(session);
                }
            }
            Job::Notice(_) => {}
            Job::Call { msg, flag } => {
                let rid = call_rid(&msg).unwrap_or(0);
                let answer = worker.call(msg, &flag);
                lock(&flags).remove(&rid);
                let mut body = frame::encode(&FromChild { rid, answer }).and_then(|b| {
                    if b.len() > MAX_FRAME {
                        Err(FrameError::TooLarge(b.len() as u64))
                    } else {
                        Ok(b)
                    }
                });
                if body.is_err() {
                    // The answer itself is over the frame limit: say so instead of dying. The
                    // side files it wrote go with it.
                    worker.discard_written();
                    let f = Failure::too_large(t!(
                        en: "the plugin's answer is too large",
                        ja: "プラグインの応答が大きすぎます"
                    ));
                    body = frame::encode(&FromChild {
                        rid,
                        answer: Answer::Err(failure_to_wire(&f)),
                    });
                }
                let wrote = body.and_then(|b| frame::write_frame(&mut output, &b));
                if let Err(e) = wrote {
                    worker.discard_written();
                    result = Err(io_err(e));
                    break;
                }
            }
        }
    }
    // Leaving: the sessions are dropped one by one in the order they were opened (a plugin may
    // keep something in a session that it flushes on drop), each shielded from the others'
    // panics, and then the plugin itself.
    worker.finish();
    // The reader may be blocked in `read`; it ends when the host closes the pipe. Do not wait for
    // it: a host that left the pipe open and went away is the OS's to clean up.
    drop(reader);
    result
}

/// Ask every running or queued call to stop.
fn raise_all(flags: &Mutex<HashMap<u64, Arc<AtomicBool>>>) {
    for f in lock(flags).values() {
        f.store(true, Ordering::SeqCst);
    }
}

/// Drop a value whose `Drop` is the plugin's code. A panic in it is the plugin's alone: it is
/// caught and reported on stderr, and the process (and every other session) goes on.
fn quiet_drop<T>(v: T) {
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| drop(v))) {
        let text = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_string());
        eprintln!("konoma plugin: a panic while dropping: {text}");
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn call_rid(m: &ToChild) -> Option<u64> {
    match m {
        ToChild::Open { rid, .. }
        | ToChild::View { rid, .. }
        | ToChild::Act { rid, .. }
        | ToChild::Key { rid, .. } => Some(*rid),
        _ => None,
    }
}

struct Worker {
    plugin: Arc<dyn Plugin>,
    action_ids: Vec<String>,
    sessions: HashMap<u64, Box<dyn Session>>,
    side: Side,
    max_sessions: usize,
    /// The side files the latest answer wrote (deleted if the answer is not sent).
    written: Vec<String>,
}

fn panic_failure(what: &str, payload: Box<dyn std::any::Any + Send>) -> Failure {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_string());
    Failure::external(t!(
        en: "the plugin crashed",
        ja: "プラグインが異常終了しました"
    ))
    .with_detail(format!("panic in {what}: {text}"))
}

fn unknown_session(sid: u64) -> Failure {
    Failure::corrupt(t!(
        en: "the host named a session that is not open",
        ja: "開いていないセッションが指定されました"
    ))
    .with_detail(format!("session {sid}"))
}

fn cancel_of(flag: &Arc<AtomicBool>) -> Cancel {
    let flag = Arc::clone(flag);
    Cancel::from_fn(move || flag.load(Ordering::SeqCst))
}

impl Worker {
    fn state(&self, session: &dyn Session) -> Result<WState, Failure> {
        catch_unwind(AssertUnwindSafe(|| WState {
            layout: layout_to_wire(&session.layout()),
            enabled: self
                .action_ids
                .iter()
                .map(|id| (id.clone(), session.enabled(id)))
                .collect(),
        }))
        .map_err(|p| panic_failure("layout/enabled", p))
    }

    fn call(&mut self, msg: ToChild, flag: &Arc<AtomicBool>) -> Answer {
        self.written.clear();
        // The names of the side files the call brings, so the ones it does not get to read are
        // deleted when it fails.
        let brought = match &msg {
            ToChild::Open { input, .. } => input_side_names(input),
            _ => Vec::new(),
        };
        let result = if flag.load(Ordering::SeqCst) {
            // Asked to stop while it waited behind other calls: the child works through its
            // calls in order, so it is only now that this one is taken, and it does not run.
            Err(Failure::cancelled(t!(
                en: "the call was cancelled before it started",
                ja: "開始前に中断されました"
            )))
        } else {
            self.call_inner(msg, flag)
        };
        match result {
            Ok(r) => Answer::Ok(r),
            Err(f) => {
                for n in &brought {
                    self.side.discard(n);
                }
                self.discard_written();
                Answer::Err(failure_to_wire(&f))
            }
        }
    }

    /// Delete the side files the latest answer wrote.
    fn discard_written(&mut self) {
        for n in self.written.drain(..) {
            self.side.discard(&n);
        }
    }

    /// Drop every session in the order they were opened, then the plugin.
    fn finish(self) {
        let Worker {
            plugin,
            mut sessions,
            ..
        } = self;
        let mut sids: Vec<u64> = sessions.keys().copied().collect();
        sids.sort_unstable();
        for sid in sids {
            if let Some(session) = sessions.remove(&sid) {
                quiet_drop(session);
            }
        }
        quiet_drop(plugin);
    }

    fn call_inner(&mut self, msg: ToChild, flag: &Arc<AtomicBool>) -> Result<Reply, Failure> {
        let cx_of = |c| -> Result<CxData, Failure> { Ok(cx_from_wire(c)?) };
        match msg {
            ToChild::Open { sid, input, cx, .. } => {
                if self.sessions.len() >= self.max_sessions && !self.sessions.contains_key(&sid) {
                    return Err(Failure::too_heavy(t!(
                        en: "the plugin has too many sessions open",
                        ja: "プラグインのセッションが多すぎます"
                    )));
                }
                let cx = cx_of(cx)?;
                let input = input_from_wire(input, &mut Dec::new(&self.side))?;
                let ocx = OpenCx::new(&cx.settings, &cx.lang)
                    .with_cancel(cancel_of(flag))
                    .with_term(cx.term)
                    .with_external(cx.external)
                    .with_read_scope(cx.read_scope.clone());
                let plugin = Arc::clone(&self.plugin);
                let session = catch_unwind(AssertUnwindSafe(|| plugin.open(input, &ocx)))
                    .map_err(|p| panic_failure("open", p))??;
                let state = match self.state(&*session) {
                    Ok(s) => s,
                    Err(f) => {
                        quiet_drop(session);
                        return Err(f);
                    }
                };
                if let Some(old) = self.sessions.insert(sid, session) {
                    quiet_drop(old);
                }
                Ok(Reply::Opened { state })
            }
            ToChild::View { sid, req, cx, .. } => {
                let cx = cx_of(cx)?;
                let req = req_from_wire(req)?;
                let vcx = ViewCx::new(&cx.settings, &cx.lang)
                    .with_cancel(cancel_of(flag))
                    .with_term(cx.term)
                    .with_external(cx.external)
                    .with_read_scope(cx.read_scope.clone());
                let session = self
                    .sessions
                    .get_mut(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                let mut view = catch_unwind(AssertUnwindSafe(|| session.view(&req, &vcx)))
                    .map_err(|p| panic_failure("view", p))??;
                let mut enc = Enc::with_total_max(&self.side, SIDE_TOTAL_MAX_FROM_CHILD);
                let converted =
                    catch_unwind(AssertUnwindSafe(|| view_to_wire(&mut view, &mut enc)));
                quiet_drop(view);
                let wv = match converted {
                    Ok(Ok(wv)) => wv,
                    Ok(Err(e)) => {
                        enc.discard();
                        return Err(Failure::from(e));
                    }
                    Err(p) => {
                        enc.discard();
                        return Err(panic_failure("styler", p));
                    }
                };
                if let WView::Text(t) = &wv {
                    if let Some(note) = &t.note {
                        eprintln!("konoma plugin: {note}");
                    }
                }
                self.written = enc.into_written();
                let session = self
                    .sessions
                    .get(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                Ok(Reply::Viewed {
                    view: wv,
                    state: self.state(&**session)?,
                })
            }
            ToChild::Act {
                sid, action, cx, ..
            } => {
                let cx = cx_of(cx)?;
                let acx = ActCx::new(&cx.settings, &cx.lang)
                    .with_cancel(cancel_of(flag))
                    .with_term(cx.term)
                    .with_external(cx.external)
                    .with_read_scope(cx.read_scope.clone());
                let session = self
                    .sessions
                    .get_mut(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                let outcome = catch_unwind(AssertUnwindSafe(|| session.act(&action, &acx)))
                    .map_err(|p| panic_failure("act", p))??;
                let session = self
                    .sessions
                    .get(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                Ok(Reply::Acted {
                    outcome: outcome_to_wire(&outcome),
                    state: self.state(&**session)?,
                })
            }
            ToChild::Key { sid, key, cx, .. } => {
                let cx = cx_of(cx)?;
                let key = key_from_wire(key);
                let acx = ActCx::new(&cx.settings, &cx.lang)
                    .with_cancel(cancel_of(flag))
                    .with_term(cx.term)
                    .with_external(cx.external)
                    .with_read_scope(cx.read_scope.clone());
                let session = self
                    .sessions
                    .get_mut(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                let outcome = catch_unwind(AssertUnwindSafe(|| session.key(&key, &acx)))
                    .map_err(|p| panic_failure("key", p))??;
                let session = self
                    .sessions
                    .get(&sid)
                    .ok_or_else(|| unknown_session(sid))?;
                Ok(Reply::Acted {
                    outcome: outcome_to_wire(&outcome),
                    state: self.state(&**session)?,
                })
            }
            _ => Err(Failure::corrupt(t!(en: "not a call"))),
        }
    }
}

const HELP: &str = "\
A konoma plugin program.

USAGE:
    <program> --serve       serve the konoma host on standard input and output
    <program> --manifest    print what the plugin declares (JSON)
    <program> --version     print the plugin id and version
    <program> --help        print this text
";

/// Write `text` to standard output. A reader that left early (`| head`) is not an error worth a
/// panic: the output is simply not wanted any more.
fn print_out(text: &str) -> ExitCode {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("konoma plugin: cannot write the output: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The `main` of a plugin program (what [`export!`](crate::export) emits).
pub fn cli_main<P: Plugin>(plugin: P) -> ExitCode {
    use std::os::unix::ffi::OsStrExt;
    // The arguments of a program are bytes, not text: `args()` would panic on one that is not UTF-8.
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // The host starts a program as `<command> <the user's args> --serve`, so `--serve` is the LAST
    // argument (a program run through an interpreter, `python3 plugin.py --serve`, has its own
    // arguments before it). Anything else is read from the first argument, as a person types it.
    let first = if args.last().map(|a| a.as_bytes()) == Some(b"--serve") {
        Some(&b"--serve"[..])
    } else {
        args.first().map(|a| a.as_bytes())
    };
    match first {
        Some(b"--serve") => match serve(plugin) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("konoma plugin: {e}");
                ExitCode::FAILURE
            }
        },
        Some(b"--manifest") => match super::manifest_to_json(&plugin.manifest()) {
            Ok(json) => print_out(&format!("{json}\n")),
            Err(e) => {
                eprintln!("konoma plugin: {e}");
                ExitCode::FAILURE
            }
        },
        Some(b"--version") => {
            let m = plugin.manifest();
            print_out(&format!("{} {}\n", m.id, m.version))
        }
        Some(b"--help" | b"-h") => print_out(HELP),
        Some(other) if !other.starts_with(b"-") => {
            eprintln!(
                "konoma plugin: opening {:?} as a standalone app is not available in this version",
                args[0]
            );
            ExitCode::from(2)
        }
        Some(_) => {
            eprintln!("konoma plugin: unknown option {:?}\n\n{HELP}", args[0]);
            ExitCode::from(2)
        }
        None => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}
