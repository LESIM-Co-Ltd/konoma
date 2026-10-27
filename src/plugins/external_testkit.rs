//! Test fixtures for the plugins that are programs of their own: a plugin that runs as a **thread**
//! behind socket pairs (no real process), the launcher that starts it, and an environment whose
//! every directory is under a test's own temporary directory.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use konoma_plugin::prelude::*;
use konoma_plugin::proc::client::{Launch, Limits, Spawned};
use konoma_plugin::proc::{serve_io, ServeOptions};
use konoma_plugin::view::ImageView;

use super::external::{Env, LauncherFactory, Spec};
use crate::i18n::Lang;
use crate::test_support::{unique_tmp, TmpDir};

/// A one-shot event, so tests order things without sleeping.
#[derive(Clone, Default)]
pub(crate) struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    pub(crate) fn set(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }

    /// Wait up to `d`; whether it was set.
    pub(crate) fn wait_for(&self, d: Duration) -> bool {
        let g = self.0 .0.lock().unwrap();
        let (g, _) = self.0 .1.wait_timeout_while(g, d, |set| !*set).unwrap();
        *g
    }
}

/// How long a test waits for something that must happen (never the thing measured).
pub(crate) const PATIENCE: Duration = Duration::from_secs(20);

/// What the child saw and can be told to do.
#[derive(Default)]
pub(crate) struct Ctl {
    /// Set when a `hang` view starts running in the child.
    pub started: Gate,
    /// Lets a hanging view go.
    pub release: Gate,
    pub opened: AtomicUsize,
}

/// The plugin the child runs: it takes `*.xd` files. What the file says is what it does:
/// `image:<path>` answers with that picture file, `hang` waits for [`Ctl::release`], anything else
/// is shown as a text that names the settings, the language and the read scope the host sent.
pub(crate) struct Demo {
    pub id: String,
    pub ctl: Arc<Ctl>,
    /// Shows pictures (a media plugin to the host) instead of text.
    pub pictures: bool,
}

impl Plugin for Demo {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id.clone(), "1.2.3")
            .name(Tr::new("Demo child"))
            .shows(if self.pictures {
                ViewKind::Image
            } else {
                ViewKind::Text
            })
            .bind(Bind::files(["*.xd"]))
            .setting(SettingDecl::new(
                "k",
                Value::Str("default".into()),
                Tr::new("a setting"),
            ))
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.ctl.opened.fetch_add(1, Ordering::SeqCst);
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(Tr::new("files only")));
        };
        let text = std::fs::read_to_string(path)?;
        Ok(Box::new(DemoSession {
            text,
            ctl: Arc::clone(&self.ctl),
        }))
    }
}

struct DemoSession {
    text: String,
    ctl: Arc<Ctl>,
}

impl Session for DemoSession {
    fn layout(&self) -> Layout {
        Layout::single()
    }

    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let body = self.text.trim();
        if let Some(p) = body.strip_prefix("image:") {
            return Ok(View::Image(ImageView::new(ImageData::File(PathBuf::from(
                p.trim(),
            )))));
        }
        if body == "hang" {
            self.ctl.started.set();
            self.ctl.release.wait_for(PATIENCE);
        }
        Ok(View::text([format!(
            "child says hello k={:?} lang={} colors={:?} scope={:?}",
            cx.settings.get("k"),
            cx.lang,
            cx.term.colors,
            cx.read_scope.dirs()
        )]))
    }
}

/// How the child behaves when it is started.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Serves the protocol.
    Serve,
    /// Starts but never answers the handshake.
    Silent,
    /// Writes bytes that are not a frame, then ends.
    Garbage,
}

/// Starts the demo plugin as a thread over socket pairs, and counts.
#[derive(Clone)]
pub(crate) struct Rig {
    pub ctl: Arc<Ctl>,
    pub launches: Arc<AtomicUsize>,
    pub kills: Arc<AtomicUsize>,
    /// The id the child declares.
    pub id: Arc<Mutex<String>>,
    pub mode: Arc<Mutex<Mode>>,
    /// The child shows pictures instead of text.
    pub pictures: bool,
}

impl Rig {
    pub(crate) fn new(id: &str) -> Rig {
        Rig {
            ctl: Arc::default(),
            launches: Arc::default(),
            kills: Arc::default(),
            id: Arc::new(Mutex::new(id.to_string())),
            mode: Arc::new(Mutex::new(Mode::Serve)),
            pictures: false,
        }
    }

    /// A child that shows pictures.
    pub(crate) fn pictures(id: &str) -> Rig {
        Rig {
            pictures: true,
            ..Rig::new(id)
        }
    }

    pub(crate) fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }

    pub(crate) fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }

    pub(crate) fn factory(&self) -> LauncherFactory {
        let rig = self.clone();
        Arc::new(move |_spec: &Spec, _exe: &Path| -> Box<dyn Launch> {
            let rig = rig.clone();
            Box::new(move || rig.spawn())
        })
    }

    fn spawn(&self) -> std::io::Result<Spawned> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let mode = *self.mode.lock().unwrap();
        let (host_w, child_r) = UnixStream::pair()?;
        let (child_w, host_r) = UnixStream::pair()?;
        let (child_err, host_err) = UnixStream::pair()?;
        let all = Arc::new(vec![
            host_w.try_clone()?,
            child_r.try_clone()?,
            child_w.try_clone()?,
            host_r.try_clone()?,
            child_err.try_clone()?,
            host_err.try_clone()?,
        ]);
        let killed = Arc::new(AtomicBool::new(false));
        let kills = Arc::clone(&self.kills);
        let kill_all = Arc::new({
            let all = Arc::clone(&all);
            move || {
                if !killed.swap(true, Ordering::SeqCst) {
                    kills.fetch_add(1, Ordering::SeqCst);
                }
                for s in all.iter() {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
        });
        let plugin = Demo {
            id: self.id.lock().unwrap().clone(),
            ctl: Arc::clone(&self.ctl),
            pictures: self.pictures,
        };
        let end = Arc::clone(&kill_all);
        // A process that exits closes its pipes; the thread does the same when it is done.
        let child_out = child_w.try_clone()?;
        std::thread::Builder::new()
            .name("test-child".into())
            .spawn(move || {
                match mode {
                    Mode::Serve => {
                        let _ = serve_io(plugin, child_r, child_w, &ServeOptions::default());
                    }
                    Mode::Silent => {
                        // Alive and silent until the host kills it.
                        let mut r = child_r;
                        let mut buf = [0u8; 64];
                        while std::io::Read::read(&mut r, &mut buf).is_ok_and(|n| n > 0) {}
                    }
                    Mode::Garbage => {
                        let mut w = child_w;
                        let _ = w.write_all(b"this is not a frame at all, not even close");
                        let _ = w.flush();
                    }
                }
                let _ = child_out.shutdown(std::net::Shutdown::Both);
                let _ = child_err.shutdown(std::net::Shutdown::Both);
                let _ = end;
            })?;
        Ok(Spawned {
            reader: Box::new(host_r),
            writer: Box::new(host_w),
            stderr: Some(Box::new(host_err)),
            kill: Box::new(move || kill_all()),
            pid: None,
        })
    }
}

/// A test's places: the folder konoma browses, the folder the "programs" are in, and the cache and
/// handoff directories. All of them go with the guard.
pub(crate) struct Lab {
    pub dir: TmpDir,
    pub progs: PathBuf,
    pub cache: PathBuf,
    pub scratch: PathBuf,
    pub work: PathBuf,
}

impl Lab {
    pub(crate) fn new(name: &str) -> Lab {
        let dir = unique_tmp(&format!("konoma_ext_{name}"));
        for sub in ["root", "progs", "cache", "scratch", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let canon = |p: PathBuf| std::fs::canonicalize(p).unwrap();
        Lab {
            progs: canon(dir.join("progs")),
            cache: canon(dir.join("cache")),
            scratch: canon(dir.join("scratch")),
            work: canon(dir.join("work")),
            dir,
        }
    }

    /// The folder konoma browses.
    pub(crate) fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.join("root")).unwrap()
    }

    /// An executable file in the programs folder (its content is only its identity).
    pub(crate) fn program(&self, name: &str, content: &str) -> PathBuf {
        let p = self.progs.join(name);
        crate::test_support::write_executable(&p, content);
        std::fs::canonicalize(p).unwrap()
    }

    /// The environment: the rig's child stands in for every program.
    pub(crate) fn env(&self, rig: &Rig) -> Env {
        Env {
            cache_dir: self.cache.clone(),
            scratch_root: self.scratch.clone(),
            work_dir: self.work.clone(),
            path_var: Some(self.progs.clone().into_os_string()),
            cwd: self.progs.clone(),
            limits: Limits::default(),
            handshake_limit: PATIENCE,
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            lang: Lang::En,
            launcher: Some(rig.factory()),
        }
    }

    /// The cache files now in the cache directory.
    pub(crate) fn cache_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&self.cache)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

/// `[plugins.<id>] command = "<command>"` (and the extra keys).
pub(crate) fn table(command: &str, extra: &[(&str, toml::Value)]) -> toml::Table {
    let mut t = toml::Table::new();
    t.insert("command".into(), toml::Value::String(command.into()));
    for (k, v) in extra {
        t.insert((*k).to_string(), v.clone());
    }
    t
}
