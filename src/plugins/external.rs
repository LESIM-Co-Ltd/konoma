//! Plugins that run as programs of their own (stage 3 of the plugin architecture; design §4-1,
//! §4-6, §4-8, §4-10, §4-12 and §5).
//!
//! Where they come from:
//!
//! - `[plugins.<id>] command = "<program>"` (and optionally `args = [...]`) in the config. The
//!   program is searched in `PATH` when it has no `/`; no shell is involved.
//! - `konoma --plugin <path>` (any number of times): a program under development.
//!
//! Each becomes an [`ExternalPlugin`], which wraps the SDK's `ProcPlugin` (the child-process client)
//! and is registered like a user's plugin (`PluginOrigin::User`): the read scope and the rule that
//! the host calls it from a worker apply as they do to every non-standard plugin.
//!
//! # Start-up
//!
//! Nothing is started when the manifest is known. The manifest of a program is kept in the cache
//! directory under the program's real path, size, modification time and inode; a hit registers the
//! plugin without running it (the child starts at the first `open`). A miss runs the program once
//! for its handshake, at most [`HANDSHAKE_LIMIT`], and saves what it declared. A program that cannot
//! be found, does not start, speaks another protocol version or is too slow is **not registered**
//! and the reason is shown once in the start-up message. The slow part runs for all of them at the
//! same time.
//!
//! # A rebuilt program
//!
//! The program's size, modification time and inode are looked at on every `open`. When they have
//! changed (a rebuild), the old child is stopped and the next one is started from the new file: the
//! user's `r` re-opens the file, so it shows the new build. A **manifest** that changed is only
//! picked up when konoma starts again (what the host registered, the keys, the bindings, stays as
//! it was; the cache is brought up to date).
//!
//! # Registries
//!
//! The registry is shared per distinct key (`shared_registry`), and the key is the list of disabled
//! ids. [`install`] returns a token naming one loaded set; `Config::plugin_disabled` ends with its
//! [marker](marker), and `build_registry` registers the set's plugins when it sees it. Another
//! configuration (another token) is another registry.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use konoma_plugin::proc::client::{
    sweep_stale_private_roots, sweep_stale_side_dirs, CommandLaunch, Config as ProcConfig, Launch,
    Limits, ProcPlugin, Spawned,
};
use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session};
use serde::{Deserialize, Serialize};

use super::PluginTables;
use crate::i18n::Lang;

/// How long the first start of a program (the handshake that tells its manifest) may take. 5 s:
/// the host waits for it before the first screen when the cache has no entry, and a program that
/// needs longer to say who it is would hold every start-up back; a cached program is not asked.
pub(crate) const HANDSHAKE_LIMIT: Duration = Duration::from_secs(5);

/// How often the run loop looks for children to stop. 30 s: a child is stopped after minutes of
/// idleness (`Limits::idle_after`), so a coarser look costs nothing and the loop does not pay for
/// it on every frame.
pub(crate) const REAP_INTERVAL: Duration = Duration::from_secs(30);

/// The most a manifest cache file may hold when it is read back. A manifest is small; a larger file
/// is not ours.
const CACHE_MAX_BYTES: u64 = 1 << 20;

/// The version of the cache file's layout.
const CACHE_FORMAT: u32 = 1;

/// The prefix of the entry `Config::plugin_disabled` ends with when external plugins were loaded.
const MARKER_PREFIX: &str = "+external:";

/// The entry of the registry key that stands for the loaded set `token`.
pub(crate) fn marker(token: &str) -> String {
    format!("{MARKER_PREFIX}{token}")
}

/// The token a marker stands for.
pub(crate) fn token_of(marker: &str) -> Option<&str> {
    marker.strip_prefix(MARKER_PREFIX)
}

// ---------------------------------------------------------------------------------------------
// What a plugin is made from
// ---------------------------------------------------------------------------------------------

/// Where an external plugin was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// `[plugins.<id>] command = ...`.
    Config(String),
    /// `konoma --plugin <path>`.
    Dev,
}

impl Source {
    fn label(&self) -> String {
        match self {
            Source::Config(id) => format!("[plugins.{id}]"),
            Source::Dev => "--plugin".to_string(),
        }
    }
}

/// One program to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Spec {
    pub source: Source,
    /// The program as written (a path, or a name searched in `PATH`).
    pub command: String,
    pub args: Vec<String>,
}

/// What is wrong with a `[plugins.<id>]` table's `command` or `args`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableProblem {
    CommandNotAString,
    CommandEmpty,
    ArgsNotAList,
    ArgNotAString,
}

/// The `command` and `args` of a table: the spec to load (`None` when the table names no program,
/// or when it is switched off or unusable) and what is wrong with it. `enabled = false` is not a
/// problem; the plugin is simply not loaded.
pub(crate) fn spec_of_table(id: &str, table: &toml::Table) -> (Option<Spec>, Vec<TableProblem>) {
    let mut problems = Vec::new();
    let Some(command) = table.get("command") else {
        if table.contains_key("args") {
            // `args` without a program is the plugin's own setting of that name (an in-process
            // plugin may declare one), not ours.
        }
        return (None, problems);
    };
    let command = match command {
        toml::Value::String(s) if s.trim().is_empty() => {
            problems.push(TableProblem::CommandEmpty);
            None
        }
        toml::Value::String(s) => Some(s.clone()),
        _ => {
            problems.push(TableProblem::CommandNotAString);
            None
        }
    };
    let mut args = Vec::new();
    match table.get("args") {
        None => {}
        Some(toml::Value::Array(items)) => {
            for item in items {
                match item {
                    toml::Value::String(s) => args.push(s.clone()),
                    _ => problems.push(TableProblem::ArgNotAString),
                }
            }
        }
        Some(_) => problems.push(TableProblem::ArgsNotAList),
    }
    let off = matches!(table.get("enabled"), Some(toml::Value::Boolean(false)));
    let spec = match (command, problems.is_empty(), off) {
        (Some(command), true, false) => Some(Spec {
            source: Source::Config(id.to_string()),
            command,
            args,
        }),
        _ => None,
    };
    (spec, problems)
}

/// The start-up message for a problem in `[plugins.<id>]`'s `command` or `args`.
pub(crate) fn table_problem_message(id: &str, p: TableProblem, lang: Lang) -> String {
    use TableProblem as P;
    match (p, lang) {
        (P::CommandNotAString, Lang::Jp) => format!("[plugins.{id}] の command は文字列(無視)"),
        (P::CommandNotAString, Lang::En) => {
            format!("[plugins.{id}] command must be a string (ignored)")
        }
        (P::CommandEmpty, Lang::Jp) => format!("[plugins.{id}] の command が空です(無視)"),
        (P::CommandEmpty, Lang::En) => format!("[plugins.{id}] command is empty (ignored)"),
        (P::ArgsNotAList, Lang::Jp) => format!("[plugins.{id}] の args は文字列の一覧(無視)"),
        (P::ArgsNotAList, Lang::En) => {
            format!("[plugins.{id}] args must be a list of strings (ignored)")
        }
        (P::ArgNotAString, Lang::Jp) => {
            format!("[plugins.{id}] の args に文字列でない要素(無視)")
        }
        (P::ArgNotAString, Lang::En) => {
            format!("[plugins.{id}] args has an entry that is not a string (ignored)")
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The program's identity and the manifest cache
// ---------------------------------------------------------------------------------------------

/// What identifies one build of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    size: u64,
    mtime_s: i64,
    mtime_ns: i64,
    ino: u64,
}

pub(crate) fn stamp_of(path: &Path) -> io::Result<Stamp> {
    let m = std::fs::metadata(path)?;
    Ok(Stamp {
        size: m.size(),
        mtime_s: m.mtime(),
        mtime_ns: m.mtime_nsec(),
        ino: m.ino(),
    })
}

/// Find the program: a name without `/` is searched in `path_var`, anything else is taken relative
/// to `cwd`. The result is the real path of an executable regular file.
pub(crate) fn resolve_program(
    command: &str,
    path_var: Option<&OsStr>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    let found = if command.contains('/') {
        let p = Path::new(command);
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        };
        if executable(&p) {
            Some(p)
        } else {
            None
        }
    } else {
        path_var.and_then(|v| {
            std::env::split_paths(v)
                .filter(|d| d.is_absolute())
                .map(|d| d.join(command))
                .find(|p| executable(p))
        })
    };
    let found = found.ok_or_else(|| format!("`{command}` was not found or is not executable"))?;
    std::fs::canonicalize(&found).map_err(|e| format!("`{command}`: {e}"))
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    format: u32,
    path: String,
    #[serde(flatten)]
    stamp: Stamp,
    manifest: serde_json::Value,
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn cache_file(dir: &Path, exe: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    dir.join(format!("{:016x}.json", fnv64(exe.as_os_str().as_bytes())))
}

/// What the cache holds for this build of the program. A file that is missing, belongs to another
/// build, or is damaged gives `None`; a damaged one is removed so the next start writes a good one.
pub(crate) fn cache_load(dir: &Path, exe: &Path, stamp: &Stamp) -> Option<Manifest> {
    let file = cache_file(dir, exe);
    let meta = std::fs::metadata(&file).ok()?;
    let parsed = (|| -> Option<(CacheFile, Manifest)> {
        if meta.len() > CACHE_MAX_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(&file).ok()?;
        let cached: CacheFile = serde_json::from_str(&text).ok()?;
        let manifest =
            konoma_plugin::proc::manifest_from_json(&cached.manifest.to_string()).ok()?;
        Some((cached, manifest))
    })();
    match parsed {
        None => {
            let _ = std::fs::remove_file(&file);
            None
        }
        Some((cached, manifest)) => (cached.format == CACHE_FORMAT
            && Some(cached.path.as_str()) == exe.to_str()
            && cached.stamp == *stamp)
            .then_some(manifest),
    }
}

/// Keep `manifest` as what this build of the program declares. A failure to write is not an error:
/// the cache only saves a start.
pub(crate) fn cache_store(dir: &Path, exe: &Path, stamp: &Stamp, manifest: &Manifest) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let (Some(path), Ok(json)) = (
        exe.to_str(),
        konoma_plugin::proc::manifest_to_json(manifest),
    ) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };
    let body = CacheFile {
        format: CACHE_FORMAT,
        path: path.to_string(),
        stamp: *stamp,
        manifest: value,
    };
    let Ok(text) = serde_json::to_string(&body) else {
        return;
    };
    if std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .is_err()
    {
        return;
    }
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| io::Write::write_all(&mut f, text.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, cache_file(dir, exe)));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------------------------
// Starting a program
// ---------------------------------------------------------------------------------------------

/// A launcher the tests put in place of a real program.
pub(crate) type LauncherFactory = Arc<dyn Fn(&Spec, &Path) -> Box<dyn Launch> + Send + Sync>;

struct BoxedLaunch(Box<dyn Launch>);

impl Launch for BoxedLaunch {
    fn launch(&self) -> io::Result<Spawned> {
        self.0.launch()
    }
}

/// Where things live and how programs are started.
#[derive(Clone)]
pub(crate) struct Env {
    /// Where the manifests are cached.
    pub cache_dir: PathBuf,
    /// Where each child's file-handoff directory is made.
    pub scratch_root: PathBuf,
    /// The working directory of every child.
    pub work_dir: PathBuf,
    pub path_var: Option<OsString>,
    /// What a relative `command` or `--plugin` path is relative to.
    pub cwd: PathBuf,
    pub limits: Limits,
    /// How long a program may take to answer the first handshake ([`HANDSHAKE_LIMIT`]).
    pub handshake_limit: Duration,
    pub host_version: String,
    pub lang: Lang,
    /// Only the tests set it: the launcher of a program that is not a real one.
    pub launcher: Option<LauncherFactory>,
}

impl Env {
    /// The real places: the cache under konoma's cache root, the handoff and working directories
    /// under the process's private directory (removed when konoma leaves).
    pub(crate) fn production(lang: Lang) -> Env {
        Env {
            cache_dir: crate::app::cache_root()
                .join("konoma")
                .join("plugin-manifests"),
            scratch_root: crate::preview::private_dir::private_dir("plugin"),
            work_dir: crate::preview::private_dir::private_dir("plugin-work"),
            path_var: std::env::var_os("PATH"),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            limits: Limits::default(),
            handshake_limit: HANDSHAKE_LIMIT,
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            lang,
            launcher: None,
        }
    }

    fn lang_code(&self) -> &'static str {
        match self.lang {
            Lang::Jp => "ja",
            Lang::En => "en",
        }
    }

    /// The configuration of every connection, however the plugin was made: `limits` as they are,
    /// including `start_timeout` (what a restart and a cached plugin's first start are given).
    /// Only the very first handshake of a program the cache knows nothing about is shorter
    /// ([`Env::handshake_limit`], passed to `ProcPlugin::start_within`).
    fn proc_config(&self) -> ProcConfig {
        ProcConfig {
            limits: self.limits.clone(),
            scratch_root: self.scratch_root.clone(),
            host_version: self.host_version.clone(),
            lang: self.lang_code().to_string(),
        }
    }

    fn launch_of(&self, spec: &Spec, exe: &Path) -> BoxedLaunch {
        BoxedLaunch(match &self.launcher {
            Some(make) => make(spec, exe),
            None => Box::new(
                CommandLaunch::new(exe, serve_args(&spec.args)).with_work_dir(&self.work_dir),
            ),
        })
    }
}

/// The arguments of a child: the user's `args`, then `--serve` (always, once, last), so that
/// `command = "python3", args = ["plugin.py"]` runs `python3 plugin.py --serve`.
fn serve_args(user: &[String]) -> Vec<OsString> {
    user.iter()
        .map(OsString::from)
        .chain(std::iter::once(OsString::from("--serve")))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The plugin the host registers
// ---------------------------------------------------------------------------------------------

/// The shutting-down flag: once set, no child is started any more (see [`shutdown`]).
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

struct Current {
    proc: Arc<ProcPlugin>,
    stamp: Stamp,
    /// The cache has to be written for this build (set when the program was rebuilt).
    cache_stale: bool,
}

/// A plugin that is a program of its own, as the host registers it.
pub(crate) struct ExternalPlugin {
    manifest: Manifest,
    spec: Spec,
    exe: PathBuf,
    env: Env,
    current: Mutex<Current>,
    /// This plugin starts no more children (set when konoma is leaving).
    closed: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Plugin for ExternalPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if self.leaving() {
            return Err(Failure::external(konoma_plugin::t!(
                en: "konoma is leaving",
                ja: "konoma を終了しています",
            )));
        }
        let (proc, store) = self.current_proc();
        let opened = proc.open(input, cx);
        if self.leaving() {
            // The exit path stopped the children before this call came back.
            proc.stop();
        }
        if let Some((stamp, learned)) = store {
            cache_store(
                &self.env.cache_dir,
                &self.exe,
                &stamp,
                learned.as_ref().unwrap_or(&self.manifest),
            );
        }
        opened
    }
}

impl ExternalPlugin {
    /// The connection to use for an `open`: the existing one, or a new one when the program was
    /// rebuilt since. The second part is what to write to the cache after the call, if anything.
    fn current_proc(&self) -> (Arc<ProcPlugin>, Option<(Stamp, Option<Manifest>)>) {
        let now = stamp_of(&self.exe).ok();
        let mut cur = lock(&self.current);
        if let Some(now) = now {
            if now != cur.stamp {
                // A rebuilt program: the old child (and the sessions on it) go, the next child is
                // the new build.
                cur.proc.stop();
                cur.proc = Arc::new(ProcPlugin::from_manifest(
                    self.manifest.clone(),
                    self.env.launch_of(&self.spec, &self.exe),
                    self.env.proc_config(),
                ));
                cur.stamp = now;
                cur.cache_stale = true;
            }
        }
        let learned = cur.proc.learned_manifest();
        let store = (cur.cache_stale || learned.is_some()).then(|| (cur.stamp, learned));
        if store.is_some() {
            cur.cache_stale = false;
        }
        (Arc::clone(&cur.proc), store)
    }

    fn leaving(&self) -> bool {
        SHUTTING_DOWN.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst)
    }

    /// Stop the child now (a later `open` starts a new one).
    pub(crate) fn stop(&self) {
        lock(&self.current).proc.stop();
    }

    /// Stop the child if it has been idle, judged at `now`. Whether it was stopped.
    pub(crate) fn reap_idle_at(&self, now: Instant) -> bool {
        let proc = Arc::clone(&lock(&self.current).proc);
        proc.reap_idle_at(now)
    }

    /// Whether a child is running now.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        lock(&self.current).proc.is_running()
    }
}

// ---------------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------------

/// A message for the start-up line, in both languages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    en: String,
    ja: String,
}

impl Problem {
    fn get(&self, lang: Lang) -> &str {
        match lang {
            Lang::Jp => &self.ja,
            Lang::En => &self.en,
        }
    }
}

/// One loaded set: what a run of konoma was asked to load, and what came of it.
struct Installed {
    plugins: Vec<Arc<ExternalPlugin>>,
    problems: Vec<Problem>,
    /// The start-up sweep of what a host that died left (see [`sweep_in_background`]); only a test
    /// waits for it.
    #[cfg_attr(not(test), allow(dead_code))]
    sweep: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Remove, off the calling thread, what hosts that died left under the handoff root: their
/// handoff directories inside it, and their whole private directories next to it (a host only
/// ever sees its own, so a crashed host's are found by the name `konoma-<kind>-<pid>` in the
/// directory that holds this process's).
pub(crate) fn sweep_in_background(
    scratch_root: &Path,
    work_dir: &Path,
) -> Option<std::thread::JoinHandle<()>> {
    let roots = (scratch_root.to_path_buf(), work_dir.to_path_buf());
    std::thread::Builder::new()
        .name("plugin-sweep".into())
        .spawn(move || {
            sweep_stale_side_dirs(&roots.0);
            // The private directories are siblings of the ones this process uses.
            for (own, kind) in [(&roots.0, "plugin"), (&roots.1, "plugin-work")] {
                if let Some(parent) = own.parent() {
                    sweep_stale_private_roots(parent, kind);
                }
            }
        })
        .ok()
}

fn installed() -> &'static Mutex<BTreeMap<String, Arc<Installed>>> {
    static T: OnceLock<Mutex<BTreeMap<String, Arc<Installed>>>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

fn installed_set(token: &str) -> Option<Arc<Installed>> {
    lock(installed()).get(token).cloned()
}

fn all_plugins() -> Vec<Arc<ExternalPlugin>> {
    lock(installed())
        .values()
        .flat_map(|i| i.plugins.iter().cloned())
        .collect()
}

/// The plugins of the set `token`, in the order they were asked for.
pub(crate) fn plugins_of(token: &str) -> Vec<Arc<ExternalPlugin>> {
    installed_set(token)
        .map(|i| i.plugins.clone())
        .unwrap_or_default()
}

/// What went wrong while loading the set `token`, for the start-up line.
pub(crate) fn problems(token: &str, lang: Lang) -> Vec<String> {
    installed_set(token)
        .map(|i| i.problems.iter().map(|p| p.get(lang).to_string()).collect())
        .unwrap_or_default()
}

/// The ids already taken by the host: the standard plugins and its internal pseudo plugins.
fn reserved_ids() -> Vec<String> {
    super::standard_manifests()
        .into_iter()
        .chain(super::internal_manifests())
        .map(|m| m.id)
        .collect()
}

fn failure_text(f: &Failure, lang: Lang) -> String {
    let code = match lang {
        Lang::Jp => "ja",
        Lang::En => "en",
    };
    let mut out = f.message.get(code).to_string();
    if let Some(d) = f.detail.as_deref().filter(|d| !d.trim().is_empty()) {
        out.push_str(": ");
        out.push_str(d.trim());
    }
    out
}

/// One program that is ready to be registered.
struct Loaded {
    plugin: Arc<ExternalPlugin>,
}

fn load_one(env: &Env, spec: &Spec) -> Result<Loaded, Problem> {
    let label = spec.source.label();
    let fail = |en: String, ja: String| Problem {
        en: format!("{label} {en}"),
        ja: format!("{label} {ja}"),
    };
    let exe = resolve_program(&spec.command, env.path_var.as_deref(), &env.cwd).map_err(|why| {
        fail(
            format!("{why} (not loaded)"),
            format!("{why}(読み込みません)"),
        )
    })?;
    let stamp = stamp_of(&exe).map_err(|e| {
        fail(
            format!("`{}`: {e} (not loaded)", spec.command),
            format!("`{}`: {e}(読み込みません)", spec.command),
        )
    })?;
    let launch = env.launch_of(spec, &exe);
    let (proc, manifest, cache_stale) = match cache_load(&env.cache_dir, &exe, &stamp) {
        Some(manifest) => (
            ProcPlugin::from_manifest(manifest.clone(), launch, env.proc_config()),
            manifest,
            false,
        ),
        None => {
            let proc = ProcPlugin::start_within(launch, env.proc_config(), env.handshake_limit)
                .map_err(|f| {
                    fail(
                        format!(
                            "`{}` did not start: {} (not loaded)",
                            spec.command,
                            failure_text(&f, Lang::En)
                        ),
                        format!(
                            "`{}` が起動しません: {}(読み込みません)",
                            spec.command,
                            failure_text(&f, Lang::Jp)
                        ),
                    )
                })?;
            let manifest = proc.manifest();
            cache_store(&env.cache_dir, &exe, &stamp, &manifest);
            (proc, manifest, false)
        }
    };
    if let Source::Config(id) = &spec.source {
        if manifest.id != *id {
            proc.stop();
            return Err(fail(
                format!(
                    "`{}` declares the id `{}`, not `{id}` (not loaded)",
                    spec.command, manifest.id
                ),
                format!(
                    "`{}` の id は `{}` で `{id}` ではありません(読み込みません)",
                    spec.command, manifest.id
                ),
            ));
        }
    }
    Ok(Loaded {
        plugin: Arc::new(ExternalPlugin {
            manifest,
            spec: spec.clone(),
            exe,
            env: env.clone(),
            current: Mutex::new(Current {
                proc: Arc::new(proc),
                stamp,
                cache_stale,
            }),
            closed: AtomicBool::new(false),
        }),
    })
}

/// Load the programs `tables` (`command = ...`) and `dev` (`--plugin`) ask for, all at once, and
/// keep them as one set. Returns the token to put in `Config::external_plugins`, or `None` when
/// nothing was asked for. An id that the host or an earlier program already has is refused (the
/// one read later loses; config entries come before `--plugin` ones).
pub(crate) fn install(env: &Env, tables: &PluginTables, dev: &[PathBuf]) -> Option<String> {
    let mut specs: Vec<Spec> = tables
        .iter()
        .filter_map(|(id, t)| spec_of_table(id, t).0)
        .collect();
    specs.extend(dev.iter().map(|p| Spec {
        source: Source::Dev,
        command: p.to_string_lossy().into_owned(),
        args: Vec::new(),
    }));
    let sweep = sweep_in_background(&env.scratch_root, &env.work_dir);
    if specs.is_empty() {
        return None;
    }
    let results: Vec<Result<Loaded, Problem>> = std::thread::scope(|scope| {
        let handles: Vec<_> = specs
            .iter()
            .map(|spec| scope.spawn(move || load_one(env, spec)))
            .collect();
        handles
            .into_iter()
            .zip(&specs)
            .map(|(h, spec)| {
                h.join().unwrap_or_else(|_| {
                    Err(Problem {
                        en: format!(
                            "{} `{}` crashed while loading",
                            spec.source.label(),
                            spec.command
                        ),
                        ja: format!(
                            "{} `{}` の読み込み中に異常終了しました",
                            spec.source.label(),
                            spec.command
                        ),
                    })
                })
            })
            .collect()
    });
    let mut taken: Vec<String> = reserved_ids();
    let mut plugins = Vec::new();
    let mut problems = Vec::new();
    for (result, spec) in results.into_iter().zip(&specs) {
        match result {
            Err(p) => problems.push(p),
            Ok(loaded) => {
                let id = loaded.plugin.manifest.id.clone();
                if taken.contains(&id) {
                    loaded.plugin.stop();
                    let label = spec.source.label();
                    problems.push(Problem {
                        en: format!(
                            "{label} `{}` has the id `{id}`, which is already taken (not loaded)",
                            spec.command
                        ),
                        ja: format!(
                            "{label} `{}` の id `{id}` は既に使われています(読み込みません)",
                            spec.command
                        ),
                    });
                } else {
                    taken.push(id);
                    plugins.push(loaded.plugin);
                }
            }
        }
    }
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let token = format!("e{}", SEQ.fetch_add(1, Ordering::Relaxed));
    lock(installed()).insert(
        token.clone(),
        Arc::new(Installed {
            plugins,
            problems,
            sweep: Mutex::new(sweep),
        }),
    );
    Some(token)
}

/// Wait for the start-up sweep of the set `token` (only a test needs to know it is done).
#[cfg(test)]
pub(crate) fn wait_sweep(token: &str) {
    let handle = installed_set(token).and_then(|i| lock(&i.sweep).take());
    if let Some(h) = handle {
        let _ = h.join();
    }
}

/// What `konoma --plugins` says about the loaded set: one line per program.
pub(crate) fn describe(token: &str) -> Vec<String> {
    plugins_of(token)
        .iter()
        .map(|p| {
            let mut line = format!(
                "{}  {}  {}",
                p.manifest.id, p.manifest.version, p.spec.command
            );
            for a in &p.spec.args {
                line.push(' ');
                line.push_str(a);
            }
            line.push_str(&format!("  ({})", p.spec.source.label()));
            line
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Lifetime
// ---------------------------------------------------------------------------------------------

/// Stop every child now (they start again at the next `open`).
pub(crate) fn stop_all() {
    for p in all_plugins() {
        p.stop();
    }
}

/// Konoma is leaving: stop every child and start no other. Called by every exit path
/// (`preview::command::exit_cleanup`). The handoff directories go with the children
/// (`ProcPlugin::stop`) and with the private directory.
pub(crate) fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    stop_all();
}

/// [`shutdown`] for one loaded set only (the process-wide flag cannot be set in a test process
/// that runs other tests at the same time).
#[cfg(test)]
pub(crate) fn shutdown_set(token: &str) {
    for p in plugins_of(token) {
        p.closed.store(true, Ordering::SeqCst);
        p.stop();
    }
}

/// Whether the process-wide shutting-down flag is up.
#[cfg(test)]
pub(crate) fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

fn reap_plugins_at(plugins: &[Arc<ExternalPlugin>], now: Instant) -> usize {
    plugins.iter().filter(|p| p.reap_idle_at(now)).count()
}

/// Stop the children that have been idle, judged at `now`; the number stopped.
pub(crate) fn reap_all_at(now: Instant) -> usize {
    reap_plugins_at(&all_plugins(), now)
}

/// [`reap_all_at`] for one loaded set.
#[cfg(test)]
pub(crate) fn reap_set_at(token: &str, now: Instant) -> usize {
    reap_plugins_at(&plugins_of(token), now)
}

/// Called by the run loop on every pass; does something only once per [`REAP_INTERVAL`], and then
/// on a thread of its own (looking at a child can wait for a start that is in progress, which the
/// loop must not).
/// The thread started, if any (the run loop lets go of it; a test joins it).
pub(crate) fn reap_if_due(now: Instant) -> Option<std::thread::JoinHandle<()>> {
    reap_if_due_with(now, || {
        reap_all_at(Instant::now());
    })
}

/// Test entry of the throttle.
#[cfg(test)]
pub(crate) fn reap_if_due_with_for_test(
    now: Instant,
    work: impl FnOnce() + Send + 'static,
) -> bool {
    reap_if_due_with(now, work).is_some()
}

/// The throttle of [`reap_if_due`], with the work given: the thread `work` runs on, if it was
/// started.
fn reap_if_due_with(
    now: Instant,
    work: impl FnOnce() + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    static BUSY: AtomicBool = AtomicBool::new(false);
    if lock(installed()).is_empty() {
        return None;
    }
    {
        let mut last = lock(&LAST);
        if last.is_some_and(|t| now.saturating_duration_since(t) < REAP_INTERVAL) {
            return None;
        }
        *last = Some(now);
    }
    if BUSY.swap(true, Ordering::SeqCst) {
        return None;
    }
    let spawned = std::thread::Builder::new()
        .name("plugin-reap".into())
        .spawn(move || {
            work();
            BUSY.store(false, Ordering::SeqCst);
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(_) => {
            BUSY.store(false, Ordering::SeqCst);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::external_testkit::Lab;
    use std::os::unix::fs::PermissionsExt;

    fn tbl(src: &str) -> toml::Table {
        src.parse().unwrap()
    }

    #[test]
    fn the_real_launcher_runs_the_program_in_the_private_work_directory_in_its_own_group() {
        use std::io::Read;
        let lab = Lab::new("real_launch");
        // The program says where it is and which group it leads, then leaves.
        let exe = lab.program("where-prog", "#!/bin/sh\npwd -P\nps -o pgid= -p $$\n");
        let mut env = Env::production(Lang::En);
        env.work_dir = lab.work.clone();
        let spec = Spec {
            source: Source::Dev,
            command: exe.to_string_lossy().into_owned(),
            args: Vec::new(),
        };
        let mut spawned = env.launch_of(&spec, &exe).launch().expect("starts");
        let mut out = String::new();
        spawned.reader.read_to_string(&mut out).unwrap();
        let pid = spawned.pid.expect("a pid");
        (spawned.kill)();
        let mut lines = out.lines();
        assert_eq!(
            std::fs::canonicalize(lines.next().unwrap()).unwrap(),
            lab.work
        );
        let pgid: u32 = lines.next().unwrap().trim().parse().unwrap();
        assert_eq!(pgid, pid, "the program leads its own group");
    }

    #[test]
    fn marker_and_token_are_inverses_and_nothing_else_is_a_token() {
        for t in ["a", "x1", "0"] {
            assert_eq!(token_of(&marker(t)), Some(t));
        }
        for m in [
            "",
            "+test:abc",
            "external:a",
            "+external",
            "markdown",
            "+Xexternal:a",
        ] {
            assert_eq!(token_of(m), None, "{m}");
        }
    }

    #[test]
    fn spec_of_table_reads_command_and_args() {
        let (spec, problems) =
            spec_of_table("p", &tbl("command = \"prog\"\nargs = [\"-a\", \"b\"]"));
        let spec = spec.unwrap();
        assert!(problems.is_empty());
        assert_eq!(spec.command, "prog");
        assert_eq!(spec.args, vec!["-a".to_string(), "b".to_string()]);
        assert_eq!(spec.source, Source::Config("p".into()));
        // `args` alone is a plugin's own setting, not ours.
        let (spec, problems) = spec_of_table("p", &tbl("args = [\"x\"]"));
        assert!(spec.is_none() && problems.is_empty());
        let (spec, problems) = spec_of_table("p", &tbl("k = 1"));
        assert!(spec.is_none() && problems.is_empty());
    }

    #[test]
    fn spec_of_table_reports_every_kind_of_wrong_value_and_loads_nothing() {
        use TableProblem as P;
        let cases: &[(&str, &[P])] = &[
            ("command = 3", &[P::CommandNotAString]),
            ("command = [\"a\"]", &[P::CommandNotAString]),
            ("command = \"\"", &[P::CommandEmpty]),
            ("command = \"  \"", &[P::CommandEmpty]),
            ("command = \"p\"\nargs = \"x\"", &[P::ArgsNotAList]),
            ("command = \"p\"\nargs = [1]", &[P::ArgNotAString]),
            (
                "command = \"p\"\nargs = [\"a\", 2, 3]",
                &[P::ArgNotAString, P::ArgNotAString],
            ),
            (
                "command = 1\nargs = 2",
                &[P::CommandNotAString, P::ArgsNotAList],
            ),
        ];
        for (src, want) in cases {
            let (spec, problems) = spec_of_table("p", &tbl(src));
            assert!(spec.is_none(), "{src}");
            assert_eq!(&problems[..], *want, "{src}");
        }
        // enabled = false is no problem; it just loads nothing.
        let (spec, problems) = spec_of_table("p", &tbl("command = \"p\"\nenabled = false"));
        assert!(spec.is_none() && problems.is_empty());
    }

    #[test]
    fn every_table_problem_has_a_message_in_both_languages_naming_the_plugin() {
        use TableProblem as P;
        for p in [
            P::CommandNotAString,
            P::CommandEmpty,
            P::ArgsNotAList,
            P::ArgNotAString,
        ] {
            for lang in [Lang::En, Lang::Jp] {
                let m = table_problem_message("my-id", p, lang);
                assert!(m.contains("[plugins.my-id]"), "{m}");
            }
        }
    }

    #[test]
    fn table_problems_checks_an_external_table_without_calling_it_unknown() {
        use crate::plugins::{table_problems, PluginTables};
        let one = |src: &str| {
            let mut all = PluginTables::new();
            all.insert("brand-new".to_string(), tbl(src));
            table_problems(&all, Lang::En)
        };
        assert!(one("command = \"prog\"").is_empty());
        assert!(
            one("command = \"prog\"\nargs = [\"a\"]\nenabled = false\nsomething = 1").is_empty()
        );
        let p = one("command = 3");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("command must be a string"), "{p:?}");
        assert!(one("command = \"\"")[0].contains("empty"));
        assert!(one("command = \"p\"\nargs = 1")[0].contains("args must be a list"));
        assert!(one("command = \"p\"\nargs = [1]")[0].contains("not a string"));
        let p = one("command = \"p\"\nenabled = \"yes\"");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("enabled"), "{p:?}");
        let p = one("command = \"p\"\nread_only = 1");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("read_only"), "{p:?}");
        let p = one("command = \"p\"\nread_dirs = \"x\"");
        assert!(!p.is_empty(), "{p:?}");
        // Without a command it is still an unknown plugin id.
        let p = one("k = 1");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(!p[0].contains("command"), "{p:?}");
        // The Japanese text carries the same problem.
        let mut all = PluginTables::new();
        all.insert("brand-new".to_string(), tbl("command = 3"));
        assert!(table_problems(&all, Lang::Jp)[0].contains("command"));
    }

    fn exe_in(lab: &Lab, name: &str, mode: u32) -> PathBuf {
        let p = lab.progs.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    #[test]
    fn resolve_program_searches_path_only_for_bare_names() {
        let lab = Lab::new("resolve");
        let exe = exe_in(&lab, "tool", 0o755);
        let canon = std::fs::canonicalize(&exe).unwrap();
        let path_var = std::env::join_paths([lab.progs.as_path()]).unwrap();
        let cwd = lab.root();
        // A bare name is looked up on PATH.
        assert_eq!(
            resolve_program("tool", Some(&path_var), &cwd).unwrap(),
            canon
        );
        // ... and nowhere else: not in the cwd, and not with no PATH.
        std::fs::write(cwd.join("local-tool"), "x").unwrap();
        std::fs::set_permissions(
            cwd.join("local-tool"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(resolve_program("local-tool", Some(&path_var), &cwd).is_err());
        assert!(resolve_program("tool", None, &cwd).is_err());
        // A relative PATH entry is ignored.
        let here = std::env::current_dir().unwrap();
        let mut up = PathBuf::new();
        for _ in here.components().skip(1) {
            up.push("..");
        }
        let rel = std::env::join_paths([up.join(lab.progs.strip_prefix("/").unwrap())]).unwrap();
        assert!(!Path::new(&rel).is_absolute());
        assert!(
            resolve_program("tool", Some(&rel), &cwd).is_err(),
            "a relative PATH entry is not searched"
        );
        // A path with a slash is taken from the cwd, or as it is when absolute.
        assert_eq!(
            resolve_program(exe.to_str().unwrap(), None, &cwd).unwrap(),
            canon
        );
        assert!(resolve_program("./local-tool", None, &cwd).is_ok());
        // Not executable, a directory, missing.
        exe_in(&lab, "plain", 0o644);
        assert!(resolve_program("plain", Some(&path_var), &cwd).is_err());
        std::fs::create_dir_all(lab.progs.join("adir")).unwrap();
        assert!(resolve_program("adir", Some(&path_var), &cwd).is_err());
        assert!(resolve_program("nothing", Some(&path_var), &cwd).is_err());
    }

    #[test]
    fn resolve_program_gives_the_real_path_through_a_symlink() {
        let lab = Lab::new("resolve_link");
        let exe = exe_in(&lab, "real", 0o755);
        let link = lab.progs.join("alias");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let got = resolve_program(link.to_str().unwrap(), None, &lab.root()).unwrap();
        assert_eq!(got, std::fs::canonicalize(&exe).unwrap());
    }

    fn sample_manifest() -> Manifest {
        Manifest::new("cache-unit", "1.0.0")
    }

    #[test]
    fn the_cache_round_trips_and_a_changed_stamp_is_a_miss() {
        let lab = Lab::new("cache_unit");
        let exe = exe_in(&lab, "prog", 0o755);
        let stamp = stamp_of(&exe).unwrap();
        assert!(cache_load(&lab.cache, &exe, &stamp).is_none());
        cache_store(&lab.cache, &exe, &stamp, &sample_manifest());
        assert_eq!(
            cache_load(&lab.cache, &exe, &stamp).unwrap().id,
            "cache-unit"
        );
        // Each part of the stamp, on its own, is a miss.
        let variants = [
            Stamp {
                size: stamp.size + 1,
                ..stamp
            },
            Stamp {
                mtime_s: stamp.mtime_s + 1,
                ..stamp
            },
            Stamp {
                mtime_ns: stamp.mtime_ns + 1,
                ..stamp
            },
            Stamp {
                ino: stamp.ino + 1,
                ..stamp
            },
        ];
        for (i, v) in variants.iter().enumerate() {
            cache_store(&lab.cache, &exe, &stamp, &sample_manifest());
            assert!(cache_load(&lab.cache, &exe, v).is_none(), "part {i}");
        }
        // A program elsewhere has its own entry.
        let other = exe_in(&lab, "other", 0o755);
        cache_store(&lab.cache, &exe, &stamp, &sample_manifest());
        assert!(cache_load(&lab.cache, &other, &stamp_of(&other).unwrap()).is_none());
    }

    #[test]
    fn a_cache_file_bigger_than_the_cap_or_of_another_format_is_not_used_and_goes() {
        let lab = Lab::new("cache_caps");
        let exe = exe_in(&lab, "prog", 0o755);
        let stamp = stamp_of(&exe).unwrap();
        cache_store(&lab.cache, &exe, &stamp, &sample_manifest());
        let file = cache_file(&lab.cache, &exe);
        let good = std::fs::read_to_string(&file).unwrap();
        // Another format number.
        let other = good.replace(&format!("\"format\":{CACHE_FORMAT}"), "\"format\":999");
        assert_ne!(other, good);
        std::fs::write(&file, &other).unwrap();
        assert!(cache_load(&lab.cache, &exe, &stamp).is_none());
        // Padded past the cap.
        std::fs::write(
            &file,
            format!("{good}{}", " ".repeat(CACHE_MAX_BYTES as usize)),
        )
        .unwrap();
        assert!(cache_load(&lab.cache, &exe, &stamp).is_none());
        assert!(!file.exists(), "an unusable file is dropped");
        // The path inside must be the program's.
        cache_store(&lab.cache, &exe, &stamp, &sample_manifest());
        let moved = std::fs::read_to_string(&file)
            .unwrap()
            .replace(exe.to_str().unwrap(), "/somewhere/else");
        std::fs::write(&file, moved).unwrap();
        assert!(cache_load(&lab.cache, &exe, &stamp).is_none());
    }

    #[test]
    fn the_cache_directory_and_file_are_private() {
        let lab = Lab::new("cache_mode");
        let exe = exe_in(&lab, "prog", 0o755);
        let dir = lab.cache.join("fresh");
        cache_store(&dir, &exe, &stamp_of(&exe).unwrap(), &sample_manifest());
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&cache_file(&dir, &exe)) & 0o077, 0);
    }
}
