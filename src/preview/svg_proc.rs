// Drawing an SVG that konoma did not write, in a process of its own.
//
// An SVG is a program for usvg and resvg, and no estimate made beforehand can bound what a crafted
// one costs: a deep nesting overflows the stack (an abort that nothing in the process can catch), a
// chain of clip paths or a huge pattern allocates gigabytes, thousands of full-canvas fills or a
// 200 KB `<text>` take tens of seconds (16 s with a Mac's 1,000 fonts). So an SVG that comes from a file (the one the user opens, an image a
// Markdown document points at, a side of a media diff) is drawn by a child process — konoma's own
// binary, started with `--internal-svg-render` — and the parent only supervises it from a worker
// thread: it kills the child when it runs past a wall-clock limit or grows past a memory limit, and
// treats a child that dies by itself (a stack overflow) as "could not be drawn". The UI never waits
// on any of it, and nothing the child does can take the parent down.
//
// konoma's own mermaid and math SVGs are not drawn here: they are trusted, and a process hop is not
// free.
//
// **One child serves many drawings, one at a time.** Starting a process costs about 11 ms and
// enumerating the system's fonts about 17 ms more, which is more than most drawings (a README
// badge takes 2 ms), so a document with fifty badges would spend its time starting processes. A
// child therefore stays alive after answering and waits for the next request; the idle ones are kept
// per executable, at most `max_children()` in all (busy or idle), and are replaced rather than
// reused whenever the supervisor had to stop one, one has grown large, or one has served enough
// (nothing a hostile drawing did can carry over to the next: a stopped child is dead). An idle child
// ends itself after `IDLE_EXIT`.
//
// The wire format is a request on the child's stdin and a response on its stdout, repeated; nothing
// else crosses (stderr is discarded, every other file descriptor is closed by the child). The parent
// reads exactly one response per request: anything the child sends while nobody asked is a protocol
// violation and ends the child (see `Link`).
//
// Memory is limited twice. The parent watches the child's resident size from outside (`RSS_LIMIT`,
// every `POLL_INTERVAL`), and the child counts its own heap (`GuardAlloc`, armed by `child_main`)
// and ends itself with a "memory" answer the moment it would pass the same limit, which also
// catches what grows between two looks of the parent (a few GB/s) and, on Linux, a refused
// allocation (`RLIMIT_AS`) that would otherwise abort the process.
//
//   request : "KSV1" u32 max_px | u32 base_len, base bytes | u64 data_len, data      (little endian)
//   response: "KSR1" u8 status (0 = drawn, else `SvgFail::code`) [| u32 w, u32 h, w*h*4 RGBA bytes]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use image::DynamicImage;

use super::svg_guard::SvgFail;

/// The argument that turns the konoma binary into the drawing process. Not in `--help`.
pub const CHILD_FLAG: &str = "--internal-svg-render";

/// Wall-clock limit for one drawing. A legitimate SVG draws in well under 100 ms (the sample
/// files and the badges of a README: 2-30 ms; a 3,500-polygon map: about 50 ms; a 3,000-node
/// mermaid-sized diagram: about 1 s); every hostile file measured needed tens of seconds or never
/// finished. 5 s is the point where a user has already given up waiting for one picture.
pub const WALL_LIMIT: Duration = Duration::from_secs(5);

/// Resident-memory limit for one drawing. The largest legitimate drawing is a 4096 x 4096 canvas:
/// the pixmap (64 MiB) and the straight-alpha copy (64 MiB) plus a few layers, a few hundred MiB
/// at most; the render budget in `svg_guard` refuses what would need more than 1 GiB of layers.
pub const RSS_LIMIT: u64 = 1 << 30;

/// A child that is larger than this when it has answered is not kept for the next drawing: it
/// would start that one with most of the limit already used (the allocator keeps what a big
/// drawing freed). A fresh child with the fonts loaded is about 100 MiB.
const RECYCLE_RSS: u64 = 300 << 20;

/// A child is replaced after this many drawings, whatever its size.
const RECYCLE_AFTER: u32 = 200;

/// How long an idle child waits for the next request before it ends itself. This is only the
/// backstop for a child whose parent is gone: while konoma is alive it retires its idle children
/// itself (`RETIRE_AFTER`), so none ends on its own and waits to be reaped.
const IDLE_EXIT: Duration = Duration::from_secs(10);

/// How long a child may sit idle before konoma stops it (shorter than `IDLE_EXIT`).
const RETIRE_AFTER: Duration = Duration::from_secs(8);

/// How often idle children are looked at (retired when old, reaped if they died).
const REAP_INTERVAL: Duration = Duration::from_millis(500);

/// How often the parent looks at the child (answer, time, memory, cancellation). A child that
/// allocates at several GB/s overshoots the memory limit by at most this long (measured in the
/// tests).
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// Largest request the child accepts: the SVG itself (`MAX_SVG_BYTES` after gunzip) and the header.
const MAX_REQUEST_BYTES: u64 = super::svg_guard::MAX_SVG_BYTES as u64 + (1 << 20);

/// Longest base directory a request may carry.
const MAX_BASE_BYTES: usize = 1 << 16;

/// Largest side of a raster the child may send back (the 4096 px cap).
const MAX_SIDE: u32 = 4096;

/// Virtual address space ceiling for the child on Linux. A backstop only: fontdb maps every
/// installed font, which counts here, so the limit has to sit far above any real font set. The
/// resident-memory watch is what enforces `RSS_LIMIT`.
#[cfg(target_os = "linux")]
const CHILD_ADDRESS_SPACE: u64 = 8 << 30;

/// The limits one supervised drawing runs under (a parameter so tests can use small ones).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub wall: Duration,
    pub rss: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            wall: WALL_LIMIT,
            rss: RSS_LIMIT,
        }
    }
}

/// What the parent asks the child to draw.
pub struct Request<'a> {
    /// The SVG (gzip is allowed: the child decompresses it, bounded).
    pub data: &'a [u8],
    /// Directory relative `<image href>` references are resolved against.
    pub base: Option<&'a Path>,
    /// Target for the longer side of the raster.
    pub max_px: u32,
}

/// Why a supervised drawing produced no picture.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RunError {
    /// The caller no longer wants it (the preview moved on); the child was stopped.
    Cancelled,
    Failed(SvgFail),
}

impl From<SvgFail> for RunError {
    fn from(f: SvgFail) -> RunError {
        RunError::Failed(f)
    }
}

// ---- children: registry, idle pool, concurrency ---------------------------------------------

/// Pids of every child alive now, busy or idle. `shutdown` kills them when konoma exits.
static LIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Children waiting for their next drawing, by executable.
static IDLE: Mutex<Option<HashMap<PathBuf, Vec<Worker>>>> = Mutex::new(None);

/// Set by `shutdown`: no new child is started once konoma is on its way out.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Test-only: the largest resident size the supervisor saw in the last drawing it supervised.
#[cfg(test)]
pub(crate) static LAST_PEAK_RSS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Number of drawings in progress now (children that are serving a request).
#[cfg(test)]
pub(crate) fn busy_children() -> usize {
    SLOTS.used.lock().map(|u| *u).unwrap_or(0)
}

/// Number of children alive now, busy or idle.
#[cfg(test)]
pub(crate) fn live_children() -> usize {
    LIVE.lock().map(|l| l.len()).unwrap_or(0)
}

/// Pids of every child alive now.
#[cfg(test)]
pub(crate) fn live_pids() -> Vec<u32> {
    LIVE.lock().map(|l| l.clone()).unwrap_or_default()
}

/// Number of waiting children that are still running and well-behaved. Looking at them reaps the
/// ones that have ended, exactly as the next drawing or the reaper would.
#[cfg(test)]
pub(crate) fn idle_alive_children() -> usize {
    IDLE.lock()
        .ok()
        .and_then(|mut g| {
            g.as_mut().map(|m| {
                m.values_mut()
                    .map(|l| l.iter_mut().filter_map(|w| w.alive().then_some(())).count())
                    .sum()
            })
        })
        .unwrap_or(0)
}

/// Number of children waiting for their next drawing.
#[cfg(test)]
pub(crate) fn idle_children() -> usize {
    IDLE.lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.values().map(Vec::len).sum()))
        .unwrap_or(0)
}

/// Kill every running child and refuse to start another. Called when konoma exits, so a hostile
/// SVG cannot go on burning CPU after the program that asked for it is gone.
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    kill_live();
}

/// Kill every child that is alive now. Busy ones are reported as a crash by their supervisor, idle
/// ones are reaped here.
pub(crate) fn kill_live() {
    // Under the lock that also guards the removal of a reaped child (`Worker::alive`, `Drop`): a pid
    // is in `LIVE` exactly as long as it has not been waited for, so it cannot have been handed to
    // an unrelated process by the time it is signalled.
    with_live(|live| {
        for &pid in live.iter() {
            kill_pid(pid);
        }
    });
    // Dropping the idle workers reaps them.
    let idle = IDLE.lock().ok().and_then(|mut g| g.take());
    drop(idle);
}

/// Run `f` on the registry of live children, holding its lock.
fn with_live<R>(f: impl FnOnce(&mut Vec<u32>) -> R) -> R {
    let mut guard = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

#[cfg(unix)]
pub(crate) fn kill_pid(pid: u32) {
    // SAFETY: plain syscall on a pid this process started and has not yet reaped (a pid leaves
    // `LIVE` in the same critical section that reaps it).
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_pid(_pid: u32) {}

/// At most this many children at once, busy and idle together: each is a full process (a font
/// database of its own, up to `RSS_LIMIT` of memory while it draws), and a Markdown file with fifty
/// badges asks for fifty drawings together.
pub(crate) fn max_children() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(2)
        .clamp(2, 4)
}

struct Slots {
    used: Mutex<usize>,
    freed: Condvar,
}

static SLOTS: Slots = Slots {
    used: Mutex::new(0),
    freed: Condvar::new(),
};

struct SlotGuard;

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Ok(mut used) = SLOTS.used.lock() {
            *used = used.saturating_sub(1);
        }
        SLOTS.freed.notify_one();
    }
}

/// Wait for a free slot, giving up when `cancelled` says so. The wait is not counted against the
/// drawing's own time limit.
fn acquire_slot(cancelled: &dyn Fn() -> bool) -> Result<SlotGuard, RunError> {
    let cap = max_children();
    let mut used = SLOTS.used.lock().map_err(|_| SvgFail::Crashed)?;
    loop {
        if SHUTTING_DOWN.load(Ordering::SeqCst) || cancelled() {
            return Err(RunError::Cancelled);
        }
        if *used < cap {
            *used += 1;
            return Ok(SlotGuard);
        }
        used = SLOTS
            .freed
            .wait_timeout(used, Duration::from_millis(20))
            .map_err(|_| SvgFail::Crashed)?
            .0;
    }
}

/// What the reader thread and the supervisor agree on about one child's output.
#[derive(Default)]
struct Link {
    /// A request has been sent whose answer has not been read yet. The supervisor sets it before it
    /// writes the request; the reader clears it when the first bytes of an answer arrive.
    expecting: AtomicBool,
    /// The child sent something nobody asked for (a second answer, or output while idle): it is
    /// not following the protocol, so it is never used again. The reader stops reading at once
    /// (a child that keeps talking cannot fill the parent's memory) and closes the pipe.
    tainted: AtomicBool,
}

/// A running child and the pipes to it. Whatever happens to the owner, the child is killed,
/// reaped and removed from the registry when this goes away (no zombie, no orphan).
struct Worker {
    child: Child,
    pid: u32,
    stdin: ChildStdin,
    /// Whole responses, framed by the reader thread. Disconnected = the child's output ended.
    answers: Receiver<Vec<u8>>,
    link: Arc<Link>,
    /// Drawings this child has answered.
    served: u32,
    /// Since when it has been waiting in the idle pool (None while it is serving a drawing).
    idle_since: Option<Instant>,
}

impl Worker {
    /// Start a child for `exe` whose heap may not pass `heap_limit` bytes.
    fn start(exe: &Path, heap_limit: u64) -> Result<Worker, SvgFail> {
        let mut child = Command::new(exe)
            .arg(CHILD_FLAG)
            .arg(heap_limit.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A crash message or a panic report has nowhere to go and nothing to say.
            .stderr(Stdio::null())
            // Not wherever the user happened to start konoma.
            .current_dir("/")
            .spawn()
            .map_err(|_| SvgFail::Crashed)?;
        let pid = child.id();
        with_live(|live| live.push(pid));
        let (Some(stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            with_live(|live| live.retain(|&p| p != pid));
            return Err(SvgFail::Crashed);
        };
        let (tx, answers) = std::sync::mpsc::channel();
        let link = Arc::new(Link::default());
        let reader_link = Arc::clone(&link);
        // Answers are read as they are produced, so a large picture cannot fill the pipe and stall
        // the child, and the supervisor can wait on a channel with a timeout. One answer per
        // request: `read_answer` refuses to read one nobody asked for.
        std::thread::spawn(move || {
            let mut asked = || {
                let ok = reader_link.expecting.swap(false, Ordering::SeqCst);
                if !ok {
                    reader_link.tainted.store(true, Ordering::SeqCst);
                }
                ok
            };
            while let Some(answer) = read_answer_if(&mut stdout, &mut asked) {
                let complete = answer_is_complete(&answer);
                if tx.send(answer).is_err() || !complete {
                    break;
                }
            }
        });
        // `shutdown` may have run between the check in `acquire_slot` and the registration.
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            kill_pid(pid);
        }
        Ok(Worker {
            child,
            pid,
            stdin,
            answers,
            link,
            served: 0,
            idle_since: None,
        })
    }

    /// Still running (not killed, not ended by its own idle timer) and still following the
    /// protocol. A child found to be gone is reaped here and leaves the registry in the same
    /// critical section, so `kill_live` can never signal a pid that has been handed on.
    fn alive(&mut self) -> bool {
        if self.link.tainted.load(Ordering::SeqCst) {
            return false;
        }
        let pid = self.pid;
        with_live(|live| match self.child.try_wait() {
            Ok(None) => true,
            _ => {
                live.retain(|&p| p != pid);
                false
            }
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let pid = self.pid;
        // Killed, reaped and unregistered under one lock (see `alive`).
        with_live(|live| {
            let _ = self.child.kill();
            let _ = self.child.wait();
            live.retain(|&p| p != pid);
        });
    }
}

/// An idle child for `exe`, if one is still alive.
fn take_idle(exe: &Path) -> Option<Worker> {
    let mut guard = IDLE.lock().ok()?;
    let list = guard.as_mut()?.get_mut(exe)?;
    while let Some(mut w) = list.pop() {
        if w.alive() {
            w.idle_since = None;
            return Some(w);
        }
    }
    None
}

/// Keep `worker` for the next drawing, if the pool has room (busy children count against the cap,
/// and this caller still holds its slot, so the pool never grows past it).
fn put_idle(exe: &Path, worker: Worker) {
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return;
    }
    start_reaper();
    let mut worker = worker;
    worker.idle_since = Some(Instant::now());
    let Ok(mut guard) = IDLE.lock() else { return };
    let map = guard.get_or_insert_with(HashMap::new);
    let idle: usize = map.values().map(Vec::len).sum();
    if idle < max_children() {
        map.entry(exe.to_path_buf()).or_default().push(worker);
    }
}

/// How long an idle child is kept: `RETIRE_AFTER`, shorter in the tests that check the retiring.
fn retire_after() -> Duration {
    #[cfg(test)]
    {
        let ms = RETIRE_AFTER_MS_FOR_TESTS.load(Ordering::SeqCst);
        if ms != 0 {
            return Duration::from_millis(ms);
        }
    }
    RETIRE_AFTER
}

#[cfg(test)]
pub(crate) static RETIRE_AFTER_MS_FOR_TESTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// One thread that looks after the idle children for the life of the process: it stops the ones
/// that have waited too long and reaps the ones that died (killed from outside, or ended by their
/// own backstop), so a dead child is never left as a zombie waiting for the next drawing.
fn start_reaper() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("svg-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(REAP_INTERVAL);
                reap_idle();
            });
    });
}

/// Drop (stop and reap) every idle child that is dead or has been idle too long.
fn reap_idle() {
    let limit = retire_after();
    let mut retired = Vec::new();
    if let Ok(mut guard) = IDLE.lock() {
        if let Some(map) = guard.as_mut() {
            for list in map.values_mut() {
                let mut keep = Vec::new();
                for mut w in list.drain(..) {
                    let old = w.idle_since.is_some_and(|t| t.elapsed() >= limit);
                    if old || !w.alive() {
                        retired.push(w);
                    } else {
                        keep.push(w);
                    }
                }
                *list = keep;
            }
        }
    }
    // Stopped and reaped outside the lock.
    drop(retired);
}

// ---- resident memory of another process -------------------------------------------------------

/// Resident set size of `pid` in bytes. `None` where it cannot be read (the caller then relies on
/// the time limit and the address-space ceiling).
#[cfg(target_os = "linux")]
pub(crate) fn rss_bytes(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf with a valid constant has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(pages * if page > 0 { page as u64 } else { 4096 })
}

#[cfg(target_os = "macos")]
pub(crate) fn rss_bytes(pid: u32) -> Option<u64> {
    // SAFETY: an all-zero `proc_taskinfo` is a valid value of a plain C struct.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `info` is a valid writable buffer of exactly `size` bytes.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (got == size).then_some(info.pti_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn rss_bytes(_pid: u32) -> Option<u64> {
    None
}

// ---- parent: supervise one drawing ------------------------------------------------------------

/// The binary that is started as the drawing process: konoma itself.
#[cfg(not(test))]
pub fn child_exe() -> Option<PathBuf> {
    self_exe_path()
}

/// A path that starts the running executable again. On Linux that is `/proc/self/exe`: a konoma
/// that has been replaced on disk while it runs (an upgrade, a rebuild) has a `current_exe()` that
/// ends in " (deleted)", which names nothing, whereas this link still reaches the program that is
/// running. Elsewhere it is `current_exe()` (macOS reports the path the program was started from,
/// and an upgrade replaces the file there; if it is removed altogether the drawing fails as
/// "stopped" until konoma restarts).
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn self_exe_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let link = PathBuf::from("/proc/self/exe");
        if std::fs::metadata(&link).is_ok() {
            return Some(link);
        }
    }
    std::env::current_exe().ok()
}

/// In the unit tests the running executable is the test harness, which does not know the flag:
/// `None` means "draw in this process" (see `svg::rasterize_untrusted`), unless the calling thread
/// has pointed this at the real binary with [`with_real_child`] (per thread, so tests running
/// beside it are not affected). The tests of the child itself start the real binary through
/// `run_with`.
#[cfg(test)]
pub fn child_exe() -> Option<PathBuf> {
    TEST_EXE.with(|e| e.borrow().clone())
}

#[cfg(test)]
thread_local! {
    static TEST_EXE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Test-only: run `f` with the production path (`svg::rasterize_untrusted`) starting `exe` as the
/// drawing process, on this thread.
#[cfg(test)]
pub(crate) fn with_real_child<T>(exe: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_EXE.with(|e| *e.borrow_mut() = Some(exe));
    let r = f();
    TEST_EXE.with(|e| *e.borrow_mut() = None);
    r
}

fn header(req: &Request) -> Vec<u8> {
    let base = req.base.map(path_bytes).unwrap_or_default();
    let mut h = Vec::with_capacity(32 + base.len());
    h.extend_from_slice(b"KSV1");
    h.extend_from_slice(&req.max_px.to_le_bytes());
    h.extend_from_slice(&(base.len() as u32).to_le_bytes());
    h.extend_from_slice(&base);
    h.extend_from_slice(&(req.data.len() as u64).to_le_bytes());
    h
}

#[cfg(unix)]
fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(p: &Path) -> Vec<u8> {
    p.to_string_lossy().into_owned().into_bytes()
}

#[cfg(unix)]
fn path_from_bytes(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

#[cfg(not(unix))]
fn path_from_bytes(b: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(b).into_owned())
}

/// How one supervised request ended.
enum Ended {
    /// The child answered. The flag says whether it can be asked again: not when it answered
    /// before it had read the whole request (it was stopped, since nothing else would end the
    /// write that is still blocked on a pipe it will never read).
    Answered(Vec<u8>, bool),
    /// The child's output ended with no (complete) answer: it died.
    Gone,
    Cancelled,
    TimedOut,
    OutOfMemory,
}

/// How long a child that has already answered may take to finish reading its request.
const WRITE_GRACE: Duration = Duration::from_millis(250);

/// Send `req` to `worker` and wait for the answer, watching the clock, the memory and `cancelled`.
/// Does not reap anything: the caller drops a worker whose request did not end in a reusable
/// `Answered`.
fn supervise(
    worker: &mut Worker,
    limits: Limits,
    req: &Request,
    cancelled: &dyn Fn() -> bool,
    peak_rss: &mut u64,
) -> Ended {
    let head = header(req);
    let started = Instant::now();
    let pid = worker.pid;
    let Worker {
        stdin,
        answers,
        link,
        ..
    } = worker;
    // Set before the request leaves, so the reader never mistakes the answer for unsolicited output.
    link.expecting.store(true, Ordering::SeqCst);
    std::thread::scope(|s| {
        // The request goes in from its own thread: a child that dies early must not leave the
        // supervisor stuck in a write into a full pipe (the write ends with an error once the
        // child is gone).
        let writer = s.spawn(move || {
            let _ = stdin
                .write_all(&head)
                .and_then(|_| stdin.write_all(req.data))
                .and_then(|_| stdin.flush());
        });
        let mut reusable = true;
        let ended = loop {
            match answers.recv_timeout(POLL_INTERVAL) {
                Ok(bytes) => break Ended::Answered(bytes, true),
                Err(RecvTimeoutError::Disconnected) => break Ended::Gone,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if cancelled() || SHUTTING_DOWN.load(Ordering::SeqCst) {
                break Ended::Cancelled;
            }
            if started.elapsed() >= limits.wall {
                break Ended::TimedOut;
            }
            if let Some(rss) = rss_bytes(pid) {
                *peak_rss = (*peak_rss).max(rss);
                if rss > limits.rss {
                    break Ended::OutOfMemory;
                }
            }
        };
        match &ended {
            Ended::Answered(..) => {
                // A child that has really answered has read the whole request, so the writer is
                // about to be done. If it is not, the child is not reading: stop it, or the join
                // below waits for as long as the child cares to live.
                let deadline = Instant::now() + WRITE_GRACE;
                while !writer.is_finished() {
                    if Instant::now() >= deadline {
                        kill_pid(pid);
                        reusable = false;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            // Stop it now so a blocked write ends and the writer thread can be joined.
            _ => kill_pid(pid),
        }
        let _ = writer.join();
        match ended {
            Ended::Answered(bytes, _) => Ended::Answered(bytes, reusable),
            other => other,
        }
    })
}

/// Draw `req` in a child process started from `exe` (or one of its idle children) and wait for the
/// picture, supervising it. The calling thread blocks (call it from a worker, never the UI
/// thread). `cancelled` is polled while waiting for a slot and while the child runs; when it turns
/// true the child is killed.
pub fn run_with(
    exe: &Path,
    limits: Limits,
    req: &Request,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RunError> {
    let _slot = acquire_slot(cancelled)?;
    loop {
        let (mut worker, reused) = match take_idle(exe) {
            Some(w) => (w, true),
            None => (Worker::start(exe, limits.rss)?, false),
        };
        let mut peak_rss = 0u64;
        let ended = supervise(&mut worker, limits, req, cancelled, &mut peak_rss);
        #[cfg(test)]
        LAST_PEAK_RSS.store(peak_rss, Ordering::SeqCst);
        return match ended {
            Ended::Cancelled => Err(RunError::Cancelled),
            Ended::TimedOut => Err(SvgFail::Timeout.into()),
            Ended::OutOfMemory => Err(SvgFail::Memory.into()),
            Ended::Gone => {
                // A reused child that was gone when it was used ended while it sat idle (its own
                // timer, a kill from outside, a thread-bound resource of the system): that says
                // nothing about this SVG, so ask the next one. Only a child started for this very
                // request counts as a verdict. (Bounded: the idle pool is.)
                if reused {
                    continue;
                }
                Err(crash_or_memory(peak_rss, limits).into())
            }
            Ended::Answered(bytes, reusable) => match parse_response(&bytes) {
                Some(result) => {
                    worker.served += 1;
                    let big = rss_bytes(worker.pid).is_some_and(|r| r > RECYCLE_RSS);
                    // A child that ended itself for memory is on its way out.
                    let spent = matches!(result, Err(SvgFail::Memory));
                    if reusable && !big && !spent && worker.served < RECYCLE_AFTER {
                        put_idle(exe, worker);
                    }
                    result.map_err(RunError::Failed)
                }
                // Not a well-formed answer: the child is not to be trusted again.
                None => Err(crash_or_memory(peak_rss, limits).into()),
            },
        };
    }
}

/// A child that died on its own: having been near the memory limit is the difference between
/// "used too much memory" and "stopped".
fn crash_or_memory(peak_rss: u64, limits: Limits) -> SvgFail {
    if peak_rss >= limits.rss / 2 {
        SvgFail::Memory
    } else {
        SvgFail::Crashed
    }
}

/// [`run_with`] with the default limits and konoma's own binary.
pub fn run(req: &Request, cancelled: &dyn Fn() -> bool) -> Result<DynamicImage, RunError> {
    let Some(exe) = child_exe() else {
        return Err(SvgFail::Crashed.into());
    };
    run_with(&exe, Limits::default(), req, cancelled)
}

/// Read one answer from the child's output: the five bytes that say what it is, then, for a
/// picture, its header and exactly its pixels. A malformed or cut-off answer is returned as far as
/// it got (and `answer_is_complete` says it is not); `None` is the end of the output with nothing
/// read.
#[cfg(test)]
fn read_answer(r: &mut impl Read) -> Option<Vec<u8>> {
    read_answer_if(r, &mut || true)
}

/// [`read_answer`], except that `asked` is consulted as soon as the first bytes of an answer have
/// arrived (before any pixel is read, so before anything large is allocated): when it says nobody
/// asked for one, nothing more is read and the result is `None`.
fn read_answer_if(r: &mut impl Read, asked: &mut dyn FnMut() -> bool) -> Option<Vec<u8>> {
    let mut out = vec![0u8; 5];
    let got = read_up_to(r, &mut out);
    if got == 0 || !asked() {
        return None;
    }
    out.truncate(got);
    if got < 5 || &out[..4] != b"KSR1" || out[4] != 0 {
        return Some(out);
    }
    let mut dims = [0u8; 8];
    let got = read_up_to(r, &mut dims);
    out.extend_from_slice(&dims[..got]);
    if got < 8 {
        return Some(out);
    }
    let w = u32::from_le_bytes(dims[..4].try_into().ok()?);
    let h = u32::from_le_bytes(dims[4..].try_into().ok()?);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Some(out);
    }
    let want = w as usize * h as usize * 4;
    let start = out.len();
    out.resize(start + want, 0);
    let got = read_up_to(r, &mut out[start..]);
    out.truncate(start + got);
    Some(out)
}

/// Fill `buf` from `r` as far as the stream goes; the number of bytes read.
fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

/// Whether `answer` is a whole, well-formed answer (so the child may be asked again).
fn answer_is_complete(answer: &[u8]) -> bool {
    parse_response(answer).is_some()
}

/// The child's answer: `None` when it is not a complete, well-formed response.
fn parse_response(b: &[u8]) -> Option<Result<DynamicImage, SvgFail>> {
    if b.len() < 5 || &b[..4] != b"KSR1" {
        return None;
    }
    let status = b[4];
    if status != 0 {
        return (b.len() == 5).then(|| Err(SvgFail::from_code(status).unwrap_or(SvgFail::Invalid)));
    }
    let w = u32::from_le_bytes(b.get(5..9)?.try_into().ok()?);
    let h = u32::from_le_bytes(b.get(9..13)?.try_into().ok()?);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let want = w as usize * h as usize * 4;
    let px = b.get(13..)?;
    if px.len() != want {
        return None;
    }
    let img = image::RgbaImage::from_raw(w, h, px.to_vec())?;
    Some(Ok(DynamicImage::ImageRgba8(img)))
}

// ---- child: the drawing process ----------------------------------------------------------------

/// Entry point of the child (`konoma --internal-svg-render <heap limit>`): read requests from stdin
/// one after another, draw each with the guarded in-process renderer, write each answer to stdout.
/// Ends when stdin does. The exit code is 0 after a clean end, 2 on a malformed request. `args` are
/// the arguments after the flag.
pub fn child_main(args: &[std::ffi::OsString]) -> i32 {
    #[cfg(unix)]
    harden();
    let heap_limit = args
        .first()
        .and_then(|a| a.to_str())
        .and_then(|a| a.parse::<usize>().ok())
        .unwrap_or(RSS_LIMIT as usize);
    arm_heap_guard(heap_limit);
    #[cfg(unix)]
    watch_parent();
    let mut stdin = std::io::stdin().lock();
    let mut out = answer_sink();
    loop {
        // Waiting for a request: if nobody asks again soon, this child is not needed.
        #[cfg(unix)]
        set_alarm(IDLE_EXIT);
        let request = match read_request(&mut stdin) {
            Ok(Some(r)) => r,
            Ok(None) => return 0,
            Err(_) => return 2,
        };
        // A drawing that runs on after the supervisor is gone ends here.
        #[cfg(unix)]
        set_alarm(WALL_LIMIT + Duration::from_secs(3));
        let answer = answer_to(&request);
        drop(request);
        if out.write_all(&answer).and_then(|_| out.flush()).is_err() {
            return 3;
        }
    }
}

/// Standard output without the line buffering `std::io::stdout` applies (a picture is binary data
/// full of newline bytes).
#[cfg(unix)]
fn answer_sink() -> impl Write {
    use std::os::fd::FromRawFd;
    // SAFETY: fd 1 is this process's stdout. The `File` owns it from here on: it closes it when the
    // child's main loop ends, which is the end of the process.
    unsafe { std::fs::File::from_raw_fd(1) }
}

#[cfg(not(unix))]
fn answer_sink() -> impl Write {
    std::io::stdout()
}

/// One drawing, as the bytes to send back.
fn answer_to(req: &WireRequest) -> Vec<u8> {
    // Fonts are read only to draw `<text>`; an SVG that mentions neither text nor an embedded image
    // (which could hold text) does not need the system's fonts enumerated.
    super::svg::set_fontless(!needs_fonts(&req.data));
    let result = std::panic::catch_unwind(|| {
        super::svg::rasterize_guarded(&req.data, req.base.as_deref(), req.max_px)
    })
    .unwrap_or(Err(SvgFail::Invalid));
    let mut out = Vec::new();
    out.extend_from_slice(b"KSR1");
    match result {
        Ok(img) => {
            let rgba = img.into_rgba8();
            out.push(0);
            out.extend_from_slice(&rgba.width().to_le_bytes());
            out.extend_from_slice(&rgba.height().to_le_bytes());
            out.extend_from_slice(rgba.as_raw());
        }
        Err(fail) => out.push(fail.code()),
    }
    out
}

/// Whether drawing `data` may read fonts: any mention of `text` (an element, with or without a
/// namespace prefix) or `image` (which can embed another SVG), or a compressed document that is not
/// looked into. An entity cannot produce markup (`svg_guard::scan_depth` refuses those).
fn needs_fonts(data: &[u8]) -> bool {
    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    data.starts_with(&[0x1f, 0x8b]) || has(data, b"text") || has(data, b"image")
}

/// A request as the child reads it.
struct WireRequest {
    max_px: u32,
    base: Option<PathBuf>,
    data: Vec<u8>,
}

/// The next request on `r`: `Ok(None)` when the stream ended before a new one began, an error for
/// one that is malformed or cut short.
fn read_request(r: &mut impl Read) -> std::io::Result<Option<WireRequest>> {
    use std::io::{Error, ErrorKind};
    let bad = || Error::new(ErrorKind::InvalidData, "malformed svg request");
    let mut magic = [0u8; 4];
    match read_up_to(r, &mut magic) {
        0 => return Ok(None),
        4 if &magic == b"KSV1" => {}
        _ => return Err(bad()),
    }
    let mut word = [0u8; 4];
    r.read_exact(&mut word)?;
    let max_px = u32::from_le_bytes(word);
    r.read_exact(&mut word)?;
    let base_len = u32::from_le_bytes(word) as usize;
    if base_len > MAX_BASE_BYTES {
        return Err(bad());
    }
    let mut base = vec![0u8; base_len];
    r.read_exact(&mut base)?;
    let mut len = [0u8; 8];
    r.read_exact(&mut len)?;
    let data_len = u64::from_le_bytes(len);
    if data_len > MAX_REQUEST_BYTES {
        return Err(bad());
    }
    let mut data = vec![0u8; data_len as usize];
    r.read_exact(&mut data)?;
    Ok(Some(WireRequest {
        max_px,
        base: (!base.is_empty()).then(|| path_from_bytes(&base)),
        data,
    }))
}

#[cfg(unix)]
fn set_alarm(after: Duration) {
    // SAFETY: plain syscall.
    unsafe {
        libc::alarm(after.as_secs().max(1) as libc::c_uint);
    }
}

/// What the child gives up before it reads a byte of the request: every inherited file descriptor
/// other than the three standard ones (it must not hold konoma's terminal, temp files or sockets),
/// core dumps (a stack-overflow crash must not write a file), scheduling priority (a hostile
/// drawing must not slow the UI), and — as a backstop under the parent's own watch — on Linux its
/// address space.
#[cfg(unix)]
fn harden() {
    // SAFETY: each call is a plain syscall with valid arguments, made before any other thread
    // exists in this process.
    unsafe {
        for fd in 3..1024 {
            libc::close(fd);
        }
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        #[cfg(target_os = "linux")]
        {
            let cap = libc::rlimit {
                rlim_cur: CHILD_ADDRESS_SPACE as libc::rlim_t,
                rlim_max: CHILD_ADDRESS_SPACE as libc::rlim_t,
            };
            libc::setrlimit(libc::RLIMIT_AS, &cap);
        }
    }
}

/// End this process when its parent is gone. (Not `PR_SET_PDEATHSIG`: on Linux that fires when the
/// *thread* that started the child exits, not the process, and konoma starts each child from a
/// short-lived worker thread. A child waiting for its next drawing already ends when the parent's
/// end of its stdin closes, and one that is drawing ends at its alarm; this ends a drawing sooner,
/// within a second.)
#[cfg(unix)]
fn watch_parent() {
    // SAFETY: plain syscall.
    let parent = unsafe { libc::getppid() };
    let _ = std::thread::Builder::new()
        .name("svg-parent-watch".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            // SAFETY: plain syscalls; `_exit` skips destructors on purpose.
            unsafe {
                if libc::getppid() != parent {
                    libc::_exit(4);
                }
            }
        });
}

// ---- child: the memory guard ---------------------------------------------------------------------

/// The heap limit of this process in bytes; 0 = no limit (every process but a drawing child).
static HEAP_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Heap bytes in use, counted only while `HEAP_LIMIT` is armed (signed: what was allocated before
/// arming and freed after counts negative, which only makes the limit a little more generous).
static HEAP_IN_USE: AtomicIsize = AtomicIsize::new(0);

fn arm_heap_guard(limit: usize) {
    HEAP_IN_USE.store(0, Ordering::SeqCst);
    HEAP_LIMIT.store(limit, Ordering::SeqCst);
}

/// The global allocator: the system's, plus — in a drawing child only — a count of the bytes in use.
/// A drawing child that would go past its limit, or whose allocation the system refuses (Linux's
/// address-space ceiling), answers "memory" and ends at once instead of growing until the parent
/// notices (a child allocates several GB/s; the parent looks every 4 ms) or aborting. Everywhere
/// else the limit is 0 and the whole cost is one relaxed load per call (not measurable: 60 million
/// allocate/free pairs took the same time with and without it).
pub struct GuardAlloc;

#[cold]
fn heap_exhausted() -> ! {
    // The answer "memory", written by hand: this runs inside the allocator, so nothing here may
    // allocate, and the parent reads it like any other answer.
    let reply = [b'K', b'S', b'R', b'1', SvgFail::Memory.code()];
    // SAFETY: plain syscalls on a static-sized buffer; `_exit` never returns.
    unsafe {
        let _ = libc::write(1, reply.as_ptr().cast(), reply.len());
        libc::_exit(5)
    }
}

// SAFETY: every operation is the system allocator's own; the additions are two atomic counters
// and, on exhaustion, a process exit.
unsafe impl GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let limit = HEAP_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return System.alloc(layout);
        }
        self.reserve(layout.size(), limit);
        let p = System.alloc(layout);
        if p.is_null() {
            heap_exhausted();
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let limit = HEAP_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return System.alloc_zeroed(layout);
        }
        self.reserve(layout.size(), limit);
        let p = System.alloc_zeroed(layout);
        if p.is_null() {
            heap_exhausted();
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if HEAP_LIMIT.load(Ordering::Relaxed) != 0 {
            HEAP_IN_USE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        }
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let limit = HEAP_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return System.realloc(ptr, layout, new_size);
        }
        if new_size > layout.size() {
            self.reserve(new_size - layout.size(), limit);
        } else {
            HEAP_IN_USE.fetch_sub((layout.size() - new_size) as isize, Ordering::Relaxed);
        }
        let p = System.realloc(ptr, layout, new_size);
        if p.is_null() {
            heap_exhausted();
        }
        p
    }
}

impl GuardAlloc {
    #[inline]
    fn reserve(&self, bytes: usize, limit: usize) {
        let now = HEAP_IN_USE.fetch_add(bytes as isize, Ordering::Relaxed) + bytes as isize;
        if now > 0 && now as usize > limit {
            heap_exhausted();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(req: &Request) -> Vec<u8> {
        let mut w = header(req);
        w.extend_from_slice(req.data);
        w
    }

    #[test]
    fn request_round_trips_through_the_wire_format() {
        let req = Request {
            data: b"<svg/>",
            base: Some(Path::new("/tmp/some dir/é")),
            max_px: 777,
        };
        let w = read_request(&mut wire(&req).as_slice()).unwrap().unwrap();
        assert_eq!(w.max_px, 777);
        assert_eq!(w.base.as_deref(), Some(Path::new("/tmp/some dir/é")));
        assert_eq!(w.data, b"<svg/>");
    }

    #[test]
    fn requests_follow_one_another_and_the_stream_end_is_not_an_error() {
        let a = Request {
            data: b"aaa",
            base: None,
            max_px: 1,
        };
        let b = Request {
            data: b"bbbbb",
            base: Some(Path::new("/x")),
            max_px: 2,
        };
        let mut both = wire(&a);
        both.extend(wire(&b));
        let mut r = both.as_slice();
        assert_eq!(read_request(&mut r).unwrap().unwrap().data, b"aaa");
        let second = read_request(&mut r).unwrap().unwrap();
        assert_eq!((second.data.as_slice(), second.max_px), (&b"bbbbb"[..], 2));
        assert!(read_request(&mut r).unwrap().is_none(), "end of stream");
    }

    #[test]
    fn malformed_requests_are_refused_not_panicked_on() {
        let refused = |b: &[u8]| read_request(&mut &b[..]).is_err();
        assert!(refused(b"KS"));
        assert!(refused(b"KSV1"));
        assert!(refused(b"XXXX\0\0\0\0\0\0\0\0\0\0\0\0"));
        // A data length that runs past the end, and one far past the limit.
        let mut w = b"KSV1".to_vec();
        w.extend_from_slice(&5u32.to_le_bytes());
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(&10u64.to_le_bytes());
        w.extend_from_slice(b"short");
        assert!(refused(&w));
        let mut w = b"KSV1".to_vec();
        w.extend_from_slice(&5u32.to_le_bytes());
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(refused(&w));
        // A base length that is absurd.
        let mut w = b"KSV1".to_vec();
        w.extend_from_slice(&5u32.to_le_bytes());
        w.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(refused(&w));
    }

    fn picture(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"KSR1\0".to_vec();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend(std::iter::repeat_n(7u8, (w * h * 4) as usize));
        b
    }

    #[test]
    fn responses_are_validated_completely() {
        assert!(parse_response(b"").is_none());
        assert!(parse_response(b"KSR1").is_none());
        assert!(parse_response(b"NOPE\0").is_none());
        assert_eq!(
            parse_response(&[b'K', b'S', b'R', b'1', SvgFail::TooDeep.code()]),
            Some(Err(SvgFail::TooDeep))
        );
        // An unknown refusal code is "invalid", never a crash.
        assert_eq!(parse_response(b"KSR1\xfe"), Some(Err(SvgFail::Invalid)));
        // A refusal followed by anything is not an answer.
        assert!(parse_response(b"KSR1\x01extra").is_none());
        // A picture whose pixel count does not match its header is not a picture.
        let mut short = picture(2, 2);
        short.pop();
        assert!(parse_response(&short).is_none());
        assert!(parse_response(&picture(2, 2)).unwrap().is_ok());
        let mut long = picture(2, 2);
        long.push(0);
        assert!(parse_response(&long).is_none());
        // Absurd dimensions are refused before any allocation.
        let mut huge = b"KSR1\0".to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_response(&huge).is_none());
    }

    #[test]
    fn answers_are_framed_one_by_one_from_a_stream() {
        let mut stream = picture(3, 2);
        stream.extend_from_slice(b"KSR1\x04");
        stream.extend(picture(1, 1));
        let mut r = stream.as_slice();
        let a = read_answer(&mut r).unwrap();
        assert_eq!(a.len(), 13 + 24);
        assert!(answer_is_complete(&a));
        assert_eq!(read_answer(&mut r).unwrap(), b"KSR1\x04");
        let c = read_answer(&mut r).unwrap();
        assert!(answer_is_complete(&c));
        assert!(read_answer(&mut r).is_none(), "end of output");
    }

    #[test]
    fn a_cut_off_or_garbled_answer_is_returned_as_far_as_it_got_and_is_not_complete() {
        for bytes in [
            &b"not a response"[..],
            b"KS",
            b"KSR1\0\x02\0\0\0\x02\0\0\0abc",
        ] {
            let a = read_answer(&mut &bytes[..]).expect("something was read");
            assert!(!answer_is_complete(&a), "{bytes:?}");
        }
        assert!(read_answer(&mut &b""[..]).is_none());
    }

    #[test]
    fn only_documents_that_can_draw_text_need_fonts() {
        assert!(needs_fonts(b"<svg><text>a</text></svg>"));
        assert!(needs_fonts(b"<svg:svg><svg:text>a</svg:text></svg:svg>"));
        assert!(needs_fonts(
            b"<svg><image href=\"data:image/svg+xml;base64,AAAA\"/></svg>"
        ));
        assert!(needs_fonts(&[0x1f, 0x8b, 8, 0]));
        assert!(!needs_fonts(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0L5 5\"/></svg>"
        ));
    }

    #[test]
    fn fail_codes_and_tags_round_trip() {
        for f in [
            SvgFail::TooDeep,
            SvgFail::TooLarge,
            SvgFail::TooComplex,
            SvgFail::TooHeavy,
            SvgFail::Timeout,
            SvgFail::Memory,
            SvgFail::Crashed,
            SvgFail::Invalid,
        ] {
            assert_eq!(SvgFail::from_code(f.code()), Some(f));
        }
        assert_eq!(SvgFail::from_code(0), None);
    }
}
