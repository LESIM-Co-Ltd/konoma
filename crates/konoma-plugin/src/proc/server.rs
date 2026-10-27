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

use super::frame::{self, FrameError, MAX_FRAME};
use super::proto::{
    Answer, FromChild, Hello, HelloReply, Reply, ToChild, WState, PROTOCOL_VERSION,
};
use super::wire::{
    self, cx_from_wire, failure_to_wire, input_from_wire, key_from_wire, layout_to_wire,
    manifest_to_wire, outcome_to_wire, req_from_wire, view_to_wire, CxData, Dec, Enc, Side,
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
}

impl Default for ServeOptions {
    fn default() -> ServeOptions {
        ServeOptions {
            max_sessions: 1024,
            side_threshold: None,
        }
    }
}

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
    use std::fs::File;
    use std::os::fd::FromRawFd;
    // SAFETY: plain fd duplication on this process's own descriptors; each new fd is wrapped in
    // exactly one `File` that owns it.
    let (input, output) = unsafe {
        let out_fd = libc::dup(1);
        let in_fd = libc::dup(0);
        if out_fd < 0 || in_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(2, 1) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
        if null >= 0 {
            libc::dup2(null, 0);
            if null > 2 {
                libc::close(null);
            }
        }
        (File::from_raw_fd(in_fd), File::from_raw_fd(out_fd))
    };
    serve_io(plugin, input, output, &ServeOptions::default())
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
            if tx.send(job).is_err() || end {
                return;
            }
        })?;

    let mut worker = Worker {
        plugin: Arc::new(plugin),
        action_ids,
        sessions: HashMap::new(),
        side,
        max_sessions: opts.max_sessions,
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
                worker.sessions.remove(&sid);
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
                    // The answer itself is over the frame limit: say so instead of dying.
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
                    result = Err(io_err(e));
                    break;
                }
            }
        }
    }
    // The reader may be blocked in `read`; it ends when the host closes the pipe. Do not wait for
    // it: a host that left the pipe open and went away is the OS's to clean up.
    drop(reader);
    result
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
        match self.call_inner(msg, flag) {
            Ok(r) => Answer::Ok(r),
            Err(f) => Answer::Err(failure_to_wire(&f)),
        }
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
                let state = self.state(&*session)?;
                self.sessions.insert(sid, session);
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
                let wv = catch_unwind(AssertUnwindSafe(|| {
                    view_to_wire(&mut view, &mut Enc::new(&self.side))
                }))
                .map_err(|p| panic_failure("styler", p))?
                .map_err(Failure::from)?;
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

/// The `main` of a plugin program (what [`export!`](crate::export) emits).
pub fn cli_main<P: Plugin>(plugin: P) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--serve") => match serve(plugin) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("konoma plugin: {e}");
                ExitCode::FAILURE
            }
        },
        Some("--manifest") => match super::manifest_to_json(&plugin.manifest()) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("konoma plugin: {e}");
                ExitCode::FAILURE
            }
        },
        Some("--version") => {
            let m = plugin.manifest();
            println!("{} {}", m.id, m.version);
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) if !other.starts_with('-') => {
            eprintln!(
                "konoma plugin: opening {other:?} as a standalone app is not available in this version"
            );
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("konoma plugin: unknown option {other:?}\n\n{HELP}");
            ExitCode::from(2)
        }
        None => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}
