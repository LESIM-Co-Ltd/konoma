//! Shared fixtures: a latch, a test plugin driven by the view request's `repr`, and a launcher
//! that runs the child as a thread over socket pairs (no real process).

#![allow(dead_code)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::proc::client::{Config, Limits, ProcPlugin, Spawned};
use konoma_plugin::proc::{serve_io, ServeOptions};
use konoma_plugin::view::*;
use konoma_plugin::*;
use konoma_test_support::TmpDir;

/// How long a test waits for something that must happen. Long: it is never the thing measured.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// A one-shot event, so tests order things without sleeping.
#[derive(Clone, Default)]
pub struct Latch(Arc<(Mutex<bool>, Condvar)>);

impl Latch {
    pub fn new() -> Latch {
        Latch::default()
    }
    pub fn set(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
    pub fn is_set(&self) -> bool {
        *self.0 .0.lock().unwrap()
    }
    /// Wait up to `d`; whether it was set.
    pub fn wait_for(&self, d: Duration) -> bool {
        let g = self.0 .0.lock().unwrap();
        let (g, _) = self.0 .1.wait_timeout_while(g, d, |set| !*set).unwrap();
        *g
    }
    pub fn wait(&self) {
        assert!(self.wait_for(PATIENCE), "a latch was never set");
    }
}

/// Poll `f` until true (for facts that settle on another thread, such as a counter).
pub fn eventually(what: &str, f: impl Fn() -> bool) {
    let until = Instant::now() + PATIENCE;
    while !f() {
        assert!(Instant::now() < until, "never became true: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// What the test plugin saw and can be told to do.
#[derive(Default)]
pub struct Ctl {
    /// Set when a "hang" call starts running in the child.
    pub started: Latch,
    /// Lets a hanging call go.
    pub release: Latch,
    pub opened: AtomicUsize,
    pub closed: AtomicUsize,
    pub last_input: Mutex<Option<String>>,
    /// The stderr stream of the current child.
    pub stderr: Mutex<Option<UnixStream>>,
    /// Ends the current child's streams ("die").
    pub die: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Ctl {
    pub fn write_stderr(&self, text: &[u8]) {
        if let Some(s) = self.stderr.lock().unwrap().as_mut() {
            let _ = s.write_all(text);
            let _ = s.flush();
        }
    }
}

pub struct TestPlugin {
    pub ctl: Arc<Ctl>,
}

impl Plugin for TestPlugin {
    fn manifest(&self) -> Manifest {
        test_manifest("test")
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        *self.ctl.last_input.lock().unwrap() = Some(describe(&input));
        if let Input::File { path, .. } = &input {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "fail-open" {
                return Err(Failure::unsupported(Tr::new("cannot open this")));
            }
            if name == "panic-open" {
                panic!("boom in open");
            }
            if name == "hang-open" {
                self.ctl.started.set();
                self.ctl.release.wait_for(PATIENCE);
            }
        }
        self.ctl.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TestSession {
            ctl: Arc::clone(&self.ctl),
            toggled: false,
        }))
    }
}

fn describe(input: &Input) -> String {
    match input {
        Input::Content {
            content_type,
            text,
            resources,
            origin,
        } => {
            let res: Vec<String> = resources
                .iter()
                .map(|(n, r)| match r {
                    Resource::Bytes(b) => format!("{n}:{}:{:?}", b.len(), b.first()),
                    _ => format!("{n}:other"),
                })
                .collect();
            format!(
                "Content {content_type} {text:?} origin={origin:?} res=[{}]",
                res.join(",")
            )
        }
        other => format!("{other:?}"),
    }
}

pub fn test_manifest(id: &str) -> Manifest {
    Manifest::new(id, "1.0.0")
        .shows(ViewKind::Text)
        .bind(Bind::files(["*.t"]))
        .action(Act::new("flash").key("f"))
        .action(Act::new("toggle").key("t"))
        .action(Act::new("gated").key("g"))
        .action(Act::new("hang").key("h"))
        .action(Act::new("panic").key("p"))
        .action(Act::new("noisy-hang").key("n"))
}

pub struct TestSession {
    ctl: Arc<Ctl>,
    toggled: bool,
}

impl Drop for TestSession {
    fn drop(&mut self) {
        self.ctl.closed.fetch_add(1, Ordering::SeqCst);
    }
}

fn text(s: impl Into<String>) -> View {
    View::Text(TextView {
        source: TextSource::Owned(s.into()),
        styler: None,
        file_lines: false,
        syntax_name: None,
    })
}

impl TestSession {
    fn hang(&self) {
        self.ctl.started.set();
        self.ctl.release.wait_for(PATIENCE);
    }
}

impl Session for TestSession {
    fn layout(&self) -> Layout {
        if self.toggled {
            Layout {
                reprs: vec![
                    ReprDecl::new("rendered", Tr::new("Rendered")),
                    ReprDecl::new("source", Tr::new("Source")),
                ],
                default_repr: Some("rendered".into()),
                ..Layout::default()
            }
        } else {
            Layout::single()
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        match req.repr.as_deref().unwrap_or("plain") {
            "plain" => Ok(text(format!(
                "hello {}x{} page {:?}",
                req.cols, req.rows, req.page
            ))),
            "hang" => {
                self.hang();
                Ok(text("late"))
            }
            "wait-cancel" => {
                self.ctl.started.set();
                let until = Instant::now() + PATIENCE;
                while !cx.cancel.is_cancelled() {
                    assert!(Instant::now() < until, "the cancel never arrived");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(Failure::cancelled(Tr::new("stopped when asked")))
            }
            "ignore-cancel" => {
                self.hang();
                Ok(text("late"))
            }
            "panic" => panic!("boom in view"),
            "die" => {
                self.ctl.write_stderr(&vec![b'e'; 20_000]);
                self.ctl.write_stderr(b"LAST-WORDS");
                if let Some(d) = self.ctl.die.lock().unwrap().as_ref() {
                    d();
                }
                std::thread::sleep(Duration::from_millis(50));
                Ok(text("unreachable"))
            }
            "canvas" => {
                let mut c = CanvasView::new(req.cols.max(5), req.rows.max(2));
                c.set_string(0, 0, "hi 日本", Style::new().bold());
                Ok(View::Canvas(c.text_input(true)))
            }
            "badcanvas" => {
                let mut c = CanvasView::new(3, 1);
                c.cells[0].symbol = "\u{7}".into();
                Ok(View::Canvas(c))
            }
            "settings" => Ok(text(format!(
                "k={:?} lang={} pixels={} colors={:?} external={} scope={:?}",
                cx.settings.get("k"),
                cx.lang,
                cx.term.pixels,
                cx.term.colors,
                cx.external,
                cx.read_scope.dirs()
            ))),
            "fail" => Err(Failure::io(Tr::new("nope").ja("だめ")).with_detail("why not")),
            "bigdoc" => {
                let mut res = Resources::new();
                res.insert("big.bin", Resource::Bytes(Arc::from(vec![7u8; 2 << 20])));
                Ok(View::Document(DocumentView {
                    markdown: "# big".into(),
                    origin: Origin::File(PathBuf::from("/x.md")),
                    resources: res,
                    options: DocOptions::default(),
                    title: None,
                    truncated: false,
                }))
            }
            "huge" => Ok(text("x".repeat(70 << 20))),
            other => Ok(text(format!("unknown repr {other}"))),
        }
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        match action {
            "flash" => Ok(Outcome::Flash(Tr::new("flashed").ja("フラッシュ"))),
            "toggle" => {
                self.toggled = !self.toggled;
                Ok(Outcome::Redraw)
            }
            "hang" => {
                self.hang();
                Ok(Outcome::Redraw)
            }
            "noisy-hang" => {
                self.ctl.write_stderr(b"MARKER-BEFORE-HANG\n");
                self.hang();
                Ok(Outcome::Redraw)
            }
            "panic" => panic!("boom in act"),
            _ => Ok(Outcome::Unhandled),
        }
    }

    fn key(&mut self, key: &Key, _cx: &ActCx) -> Result<Outcome, Failure> {
        if key.code == KeyCode::Char('!') {
            self.hang();
        }
        Ok(Outcome::Flash(Tr::new(format!(
            "key {:?} {:?}",
            key.code, key.mods
        ))))
    }

    fn enabled(&self, action: &str) -> bool {
        action != "gated" || self.toggled
    }
}

/// Launches the test plugin as a thread over socket pairs, and counts.
#[derive(Clone)]
pub struct Rig {
    pub ctl: Arc<Ctl>,
    pub launches: Arc<AtomicUsize>,
    pub kills: Arc<AtomicUsize>,
    /// The id the next child declares (to test a mismatch).
    pub id: Arc<Mutex<String>>,
    pub side_threshold: Option<usize>,
    /// The pid the host watches the memory of.
    pub pid: Option<u32>,
}

impl Rig {
    pub fn new() -> Rig {
        Rig {
            ctl: Arc::new(Ctl::default()),
            launches: Arc::default(),
            kills: Arc::default(),
            id: Arc::new(Mutex::new("test".into())),
            side_threshold: None,
            pid: None,
        }
    }

    pub fn launch(&self) -> impl Fn() -> std::io::Result<Spawned> + Send + Sync + 'static {
        let rig = self.clone();
        move || rig.spawn()
    }

    fn spawn(&self) -> std::io::Result<Spawned> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let (host_w, child_r) = UnixStream::pair()?;
        let (child_w, host_r) = UnixStream::pair()?;
        let (child_err, host_err) = UnixStream::pair()?;
        let all = vec![
            host_w.try_clone()?,
            child_r.try_clone()?,
            child_w.try_clone()?,
            host_r.try_clone()?,
            child_err.try_clone()?,
            host_err.try_clone()?,
        ];
        let killed = Arc::new(AtomicBool::new(false));
        let kill_all = {
            let all = Arc::new(all);
            let kills = Arc::clone(&self.kills);
            let killed = Arc::clone(&killed);
            move || {
                if !killed.swap(true, Ordering::SeqCst) {
                    kills.fetch_add(1, Ordering::SeqCst);
                }
                for s in all.iter() {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
        };
        let kill_all = Arc::new(kill_all);
        *self.ctl.stderr.lock().unwrap() = Some(child_err.try_clone()?);
        {
            let k = Arc::clone(&kill_all);
            *self.ctl.die.lock().unwrap() = Some(Box::new(move || k()));
        }
        let ctl = Arc::clone(&self.ctl);
        let id = self.id.lock().unwrap().clone();
        let opts = ServeOptions {
            side_threshold: self.side_threshold,
            ..ServeOptions::default()
        };
        let end = Arc::clone(&kill_all);
        std::thread::Builder::new()
            .name("test-child".into())
            .spawn(move || {
                let plugin = IdPlugin {
                    inner: TestPlugin { ctl },
                    id,
                };
                let _ = serve_io(plugin, child_r, child_w, &opts);
                // The program has exited: its ends of the pipes close.
                for s in [&child_err] {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
                let _ = end;
            })?;
        let k = Arc::clone(&kill_all);
        Ok(Spawned {
            reader: Box::new(host_r),
            writer: Box::new(host_w),
            stderr: Some(Box::new(host_err)),
            kill: Box::new(move || k()),
            pid: self.pid,
        })
    }

    pub fn plugin(&self, limits: Limits, scratch: &TmpDir) -> ProcPlugin {
        ProcPlugin::start(self.launch(), config(limits, scratch)).expect("the child starts")
    }
}

/// The test plugin under another id.
pub struct IdPlugin {
    pub inner: TestPlugin,
    pub id: String,
}

impl Plugin for IdPlugin {
    fn manifest(&self) -> Manifest {
        test_manifest(&self.id)
    }
    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.inner.open(input, cx)
    }
}

pub fn config(limits: Limits, scratch: &TmpDir) -> Config {
    std::fs::create_dir_all(scratch.as_path()).unwrap();
    Config {
        limits,
        scratch_root: scratch.as_path().to_path_buf(),
        host_version: "0.0.0-test".into(),
        lang: "en".into(),
    }
}

/// Limits short enough that a deadline test is quick, long enough that an honest call is not
/// close to them. (No test orders events by these; they only bound a hang.)
pub fn short_limits() -> Limits {
    Limits {
        open_view: Duration::from_millis(400),
        act_key: Duration::from_millis(400),
        cancel_grace: Duration::from_millis(300),
        idle_after: Duration::from_secs(300),
        start_timeout: Duration::from_secs(5),
        stderr_tail: 4096,
        rss_limit: u64::MAX,
        rss_interval: Duration::from_millis(20),
    }
}

/// Limits for tests that must not hit a deadline at all.
pub fn patient_limits() -> Limits {
    Limits {
        open_view: PATIENCE,
        act_key: PATIENCE,
        cancel_grace: PATIENCE,
        start_timeout: PATIENCE,
        ..short_limits()
    }
}

pub fn file_input(path: impl Into<PathBuf>) -> Input {
    Input::File {
        path: path.into(),
        hint: None,
    }
}

pub fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}
