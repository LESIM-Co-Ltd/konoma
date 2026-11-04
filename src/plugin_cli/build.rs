//! Building konoma with the listed plugins, and putting the result in place.
//!
//! The flow of [`rebuild`]: generate the project in a work directory under the data directory (a
//! temporary one by default), restore the saved `Cargo.lock`, optionally `cargo update`, then
//! `cargo build --release`. Only when that succeeds *and* the new program starts (`--plugins`: the
//! id checks of `Host::run`) is anything replaced: the program, its record, the lock and, for
//! install and remove, `plugins.toml`. On success, failure or interruption the work directory is
//! removed, unless `[plugins] keep_build_cache = true` asks to keep it. A separate re-exec watcher
//! stops every job's process group even if konoma is killed without running cleanup; `plugin clean`
//! removes any work directory left by such a kill.

use std::ffi::{OsStr, OsString};
#[cfg(test)]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Output};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::i18n::{tr, tr_format, Lang, Msg};
use anyhow::{anyhow, bail, Context, Result};

use super::list_file::{write_atomic_lang, PluginList};
use super::project::{generate_lang, locked_versions};
use super::switch::{self, Record, SWITCHED_ENV};

/// The build was cut short by SIGINT, SIGTERM or SIGHUP.
#[derive(Debug)]
pub struct Interrupted(pub Lang);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", tr(self.0, Msg::PluginInterrupted))
    }
}

impl std::error::Error for Interrupted {}

/// How long the new program gets to answer `--plugins`.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(60);

/// Which dependencies `cargo update` moves before the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// None: the saved `Cargo.lock` decides (install, remove).
    Nothing,
    /// Every dependency, within the semver ranges.
    All,
    /// One crate (`cargo update -p`).
    One(String),
}

/// Where things are and what the process was asked.
pub struct Env {
    /// `<config>/konoma`: `plugins.toml` and `config.toml` live here.
    pub config_dir: PathBuf,
    /// `<data>/konoma`: the rebuilt program, its record, `Cargo.lock`, the build cache.
    pub data_dir: PathBuf,
    pub home: Option<PathBuf>,
    /// The value of `$CARGO`, and of `$PATH`, for finding cargo.
    pub cargo_env: Option<OsString>,
    pub path_env: Option<OsString>,
    /// The version of the running konoma.
    pub konoma_version: String,
    pub lang: crate::i18n::Lang,
    pub stdin_is_tty: bool,
    /// Set by SIGINT, SIGTERM and SIGHUP.
    pub interrupted: Arc<AtomicBool>,
}

impl Env {
    pub fn plugins_toml(&self) -> PathBuf {
        self.config_dir.join("plugins.toml")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.data_dir.join("Cargo.lock")
    }

    /// `[plugins] keep_build_cache = true` in `config.toml`. A config that is missing or does not
    /// parse says "no": konoma reports a broken config itself when it starts.
    pub fn keep_build_cache(&self) -> bool {
        std::fs::read_to_string(self.config_dir.join("config.toml"))
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|t| t.get("plugins")?.get("keep_build_cache")?.as_bool())
            .unwrap_or(false)
    }
}

/// The cargo to run: `$CARGO` when set (it must exist), else `cargo` found in `$PATH`.
pub fn find_cargo(cargo_env: Option<&OsStr>, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let candidate = if let Some(c) = cargo_env.filter(|c| !c.is_empty()) {
        let p = PathBuf::from(c);
        // A bare name is looked up in PATH like any command.
        if p.components().count() > 1 || p.is_absolute() {
            p.is_file().then_some(p)
        } else {
            find_in_path(c, path_env)
        }
    } else {
        find_in_path(OsStr::new("cargo"), path_env)
    }?;
    // Cargo runs in the generated project, not in the directory where it was looked up.
    std::path::absolute(candidate).ok()
}

fn find_in_path(name: &OsStr, path_env: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_env?)
        .map(|d| d.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// What `install` and `update` say when there is no cargo.
pub fn no_cargo_message_lang(lang: Lang) -> String {
    tr(lang, Msg::PluginCargoMissing).to_owned()
}

/// The work directory; removed when dropped unless it is the kept build cache.
struct WorkDir {
    path: PathBuf,
    keep: bool,
}

impl WorkDir {
    fn create_lang(data_dir: &Path, keep: bool, lang: Lang) -> Result<WorkDir> {
        Self::create_with_lang(data_dir, keep, lang, |path| std::fs::create_dir_all(path))
    }

    fn create_with_lang(
        data_dir: &Path,
        keep: bool,
        lang: Lang,
        create: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<WorkDir> {
        let path = if keep {
            data_dir.join("build")
        } else {
            data_dir.join(format!("build-{}", std::process::id()))
        };
        let existed = path.exists();
        let mut work = WorkDir {
            path,
            keep: keep && existed,
        };
        create(&work.path.join("src")).with_context(|| {
            tr(lang, Msg::PluginCannotCreate).replace("{path}", &work.path.display().to_string())
        })?;
        work.keep = keep;
        Ok(work)
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Make SIGINT, SIGTERM and SIGHUP set `interrupted` instead of ending the process. Called when
/// the build (or the removal of the rebuilt program) starts and not before: until then (the
/// confirmation question, waiting for another run's lock) nothing is running that needs an orderly
/// stop, so Ctrl-C must simply end the command.
///
/// SIGHUP (the terminal was closed) is among them because the children run in groups of their own
/// and so never get the terminal's hang-up: only this process can pass it on.
pub(super) fn catch_interrupts(interrupted: &Arc<AtomicBool>) {
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        let _ = signal_hook::flag::register(sig, Arc::clone(interrupted));
    }
}

/// How long the processes of a stopped child get to end after SIGTERM before SIGKILL.
pub(super) const STOP_GRACE: Duration = Duration::from_secs(3);

static METADATA_INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn metadata_interrupted(_: libc::c_int) {
    METADATA_INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Metadata is a CLI-only phase. Restore the prior handlers before the confirmation question;
/// signal-hook's persistent build handlers are not installed until the later build phase.
struct MetadataSignals(Vec<(libc::c_int, libc::sigaction)>);

impl MetadataSignals {
    fn install() -> std::io::Result<Self> {
        METADATA_INTERRUPTED.store(false, Ordering::SeqCst);
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: both sigactions are initialized, and the handler only stores an atomic flag.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                let mut previous = std::mem::zeroed();
                action.sa_sigaction = metadata_interrupted as *const () as usize;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(signal, &action, &mut previous) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                guard.0.push((signal, previous));
            }
        }
        Ok(guard)
    }
}

impl Drop for MetadataSignals {
    fn drop(&mut self) {
        for (signal, previous) in self.0.iter().rev() {
            // SAFETY: restore the action saved for this signal; no handler-owned memory is freed.
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
    }
}

/// Raw groups are used only by supervision fixtures. Runtime jobs use the same re-exec watcher
/// on macOS and Linux, so the entire group is stopped even when konoma cannot run cleanup.
#[cfg(test)]
fn in_own_group(cmd: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0)
}

/// Run cargo in `dir` with its output going to the terminal.
fn run_cargo(
    cargo: &Path,
    args: &[&str],
    dir: &Path,
    target: &Path,
    interrupted: &AtomicBool,
    lang: Lang,
) -> Result<ExitStatus> {
    let mut launch = super::watcher::Launch::new_lang(cargo, lang)?;
    launch
        .command()
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target)
        .stdin(Stdio::null());
    launch.spawn()?.wait(interrupted)
}

/// Run the new program with `--plugins`; an error carries what it printed.
fn verify(bin: &Path, own: &str, interrupted: &AtomicBool, lang: Lang) -> Result<()> {
    verify_with_timeout_lang(bin, own, interrupted, VERIFY_TIMEOUT, lang)
}

fn verify_with_timeout_lang(
    bin: &Path,
    own: &str,
    interrupted: &AtomicBool,
    timeout: Duration,
    lang: Lang,
) -> Result<()> {
    let started = Instant::now();
    let mut launch = super::watcher::Launch::new_lang(bin, lang)?;
    launch
        .command()
        .arg("--plugins")
        // So the new program does not hand over to the one installed now.
        .env(SWITCHED_ENV, own)
        .env(
            super::VERIFICATION_LANG_ENV,
            if lang == Lang::Jp { "jp" } else { "en" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = launch.spawn()?;
    let output = child
        .output(interrupted, Some(timeout.saturating_sub(started.elapsed())))
        .map_err(|e| {
            if e.is::<Interrupted>() {
                e
            } else {
                e.context(tr(lang, Msg::PluginVerifyIncomplete))
            }
        })?;
    if !output.status.success() {
        let out = String::from_utf8_lossy(&output.stdout);
        let err = String::from_utf8_lossy(&output.stderr);
        let why = if err.trim().is_empty() { out } else { err };
        bail!(tr(lang, Msg::PluginVerifyRefused).replace("{detail}", why.trim_end()));
    }
    Ok(())
}

#[cfg(test)]
fn collect_output(
    child: &mut std::process::Child,
    interrupted: &AtomicBool,
    timeout: Option<Duration>,
) -> Result<Output> {
    match konoma_plugin::proc::process::collect_output(
        child,
        || interrupted.load(Ordering::SeqCst),
        timeout,
        usize::MAX,
        STOP_GRACE,
    ) {
        Ok(capture) => Ok(capture.output),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            Err(Interrupted(Lang::En).into())
        }
        Err(error) => Err(error.into()),
    }
}

/// Restore the saved lock; only a missing file permits fresh dependency resolution.
fn restore_lock(env: &Env, lock_in: &Path) -> Result<()> {
    match std::fs::copy(env.lock_path(), lock_in) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match std::fs::remove_file(lock_in) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
        Err(e) => {
            return Err(e).context(crate::i18n::tr(
                env.lang,
                crate::i18n::Msg::PluginLockRestore,
            ))
        }
    }
    Ok(())
}

/// Resolve metadata without executing build scripts, before asking whether to trust a crate.
pub(super) fn authors(env: &Env, list: &PluginList, name: &str) -> Result<Vec<String>> {
    let cargo = find_cargo(env.cargo_env.as_deref(), env.path_env.as_deref())
        .ok_or_else(|| anyhow!(no_cargo_message_lang(env.lang)))?;
    // Metadata has a work directory too. Release this lock and remove that directory before the
    // trust question; rebuild takes the same lock again after confirmation.
    let _lock = super::write_lock(env)?;
    let _signals = MetadataSignals::install()?;
    let project = generate_lang(list, &env.konoma_version, env.home.as_deref(), env.lang)?;
    let work = WorkDir::create_lang(&env.data_dir, false, env.lang)?;
    std::fs::write(work.path.join("Cargo.toml"), project.cargo_toml)?;
    std::fs::write(work.path.join("src/main.rs"), project.main_rs)?;
    restore_lock(env, &work.path.join("Cargo.lock"))?;
    let mut launch = super::watcher::Launch::new_lang(cargo, env.lang)?;
    launch
        .command()
        .args(["metadata", "--format-version", "1"])
        .current_dir(&work.path)
        .env("CARGO_TARGET_DIR", work.path.join("target"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = launch.spawn()?.output(&METADATA_INTERRUPTED, None)?;
    if !output.status.success() {
        bail!(
            "{}: {}",
            crate::i18n::tr(env.lang, crate::i18n::Msg::PluginMetadataFailed),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let authors = super::project::metadata_authors_lang(
        &String::from_utf8(output.stdout).map_err(|error| {
            if env.lang == Lang::En {
                anyhow::Error::new(error)
            } else {
                anyhow!(tr(env.lang, Msg::PluginMetadataUtf8))
            }
        })?,
        name,
        env.lang,
    )?;
    if METADATA_INTERRUPTED.load(Ordering::SeqCst) {
        return Err(Interrupted(env.lang).into());
    }
    Ok(authors)
}

/// Build konoma with the plugins in `list` and replace the installed program with it.
/// `write_list` also writes `plugins.toml` (install and remove change it; update does not).
pub fn rebuild(env: &Env, list: &PluginList, update: Update, write_list: bool) -> Result<()> {
    let cargo = find_cargo(env.cargo_env.as_deref(), env.path_env.as_deref())
        .ok_or_else(|| anyhow!(no_cargo_message_lang(env.lang)))?;
    catch_interrupts(&env.interrupted);
    let project = generate_lang(list, &env.konoma_version, env.home.as_deref(), env.lang)?;
    let work = WorkDir::create_lang(&env.data_dir, env.keep_build_cache(), env.lang)?;
    std::fs::write(work.path.join("Cargo.toml"), &project.cargo_toml)?;
    std::fs::write(work.path.join("src").join("main.rs"), &project.main_rs)?;
    // The saved lock decides the versions; without one the work directory starts clean (a kept
    // build cache may hold the lock of an earlier, since removed, build).
    let lock_in = work.path.join("Cargo.lock");
    restore_lock(env, &lock_in)?;
    let target = work.path.join("target");
    let flag = env.interrupted.as_ref();

    match &update {
        Update::Nothing => {}
        Update::All => run_checked(&cargo, &["update"], &work.path, &target, flag, env.lang)?,
        Update::One(name) => run_checked(
            &cargo,
            &["update", "-p", name],
            &work.path,
            &target,
            flag,
            env.lang,
        )?,
    }
    println!(
        "{}",
        tr(env.lang, Msg::PluginBuilding).replace("{n}", &list.plugins.len().to_string())
    );
    run_checked(
        &cargo,
        &["build", "--release"],
        &work.path,
        &target,
        flag,
        env.lang,
    )?;

    let built = target.join("release").join("konoma");
    if !built.is_file() {
        bail!(tr(env.lang, Msg::PluginBuildMissing).replace("{path}", &built.display().to_string()));
    }
    verify(&built, &env.konoma_version, flag, env.lang)?;

    let lock_text = std::fs::read_to_string(&lock_in).unwrap_or_default();
    let versions = locked_versions(&lock_text);
    let konoma_version = versions
        .get("konoma")
        .cloned()
        .unwrap_or_else(|| env.konoma_version.clone());
    let record = Record {
        konoma_version,
        plugins: versions
            .into_iter()
            .filter(|(n, _)| {
                list.plugins.contains_key(n)
                    || (super::project::STANDARD_CRATES.contains(&n.as_str())
                        && n != "konoma"
                        && n != "konoma-plugin")
            })
            .collect(),
    };
    // From here on the signals are only noticed, not obeyed: stopping half way through the
    // replacement is the one thing that would leave the pieces out of step.
    commit(
        env,
        &built,
        &record,
        (!lock_text.is_empty()).then_some(lock_text.as_str()),
        write_list.then_some(list),
    )?;
    println!(
        "{}",
        tr(env.lang, Msg::PluginInstalled).replace(
            "{path}",
            &switch::binary_path(&env.data_dir).display().to_string()
        )
    );
    Ok(())
}

const STEP_LOCK: &str = "Cargo.lock";
const STEP_LIST: &str = "plugins.toml";
const STEP_RECORD: &str = "the record of the program";
const STEP_PROGRAM: &str = "the konoma program";

/// Put a build that started fine in place of the installed one: the saved lock, `plugins.toml`
/// (`list`, when the command changes it), the record, and the program.
///
/// The pieces cannot be replaced as one; whatever stops the sequence half way leaves some new and
/// some old. The order makes that the least harmful kind of disagreement:
///
/// 1. The program is copied next to its place first. That is the big write, so a full disk shows
///    up while nothing at all has changed.
/// 2. Then the small files, `Cargo.lock`, `plugins.toml`, the record. These only ever run *ahead*
///    of the program: what the user asked for (the list, the lock) is saved, the program that is
///    running is the old one and keeps working. `konoma plugin update` rebuilds from the list and
///    puts everything in line, so one command repairs every one of these states. (The other way
///    round, a new program under an old list would be one that `list` and `remove` do not know of.)
/// 3. The program is renamed into place last: it is the step that changes what runs, and it
///    cannot be undone. Once it has happened nothing is left to fail.
fn commit(
    env: &Env,
    built: &Path,
    record: &Record,
    lock: Option<&str>,
    list: Option<&PluginList>,
) -> Result<()> {
    let staged = StagedProgram::new_lang(&env.data_dir, built, env.lang)?;
    let mut plan = Vec::new();
    if lock.is_some() {
        plan.push(STEP_LOCK);
    }
    if list.is_some() {
        plan.push(STEP_LIST);
    }
    plan.push(STEP_RECORD);
    plan.push(STEP_PROGRAM);
    let mut done = 0;
    let mut step = |r: Result<()>| -> Result<()> {
        match r {
            Ok(()) => {
                done += 1;
                Ok(())
            }
            Err(e) => Err(unfinished_lang(&e, &plan, done, env.lang)),
        }
    };
    if let Some(text) = lock {
        step(write_atomic_lang(
            &env.lock_path(),
            text.as_bytes(),
            env.lang,
        ))?;
    }
    if let Some(list) = list {
        step(list.save_lang(&env.plugins_toml(), env.lang))?;
    }
    step(record.save_lang(&env.data_dir, env.lang))?;
    step(staged.commit())
}

/// The message for a replacement that stopped: which of `plan` were done, which were not, and the
/// way to repair it.
fn unfinished_lang(cause: &anyhow::Error, plan: &[&str], done: usize, lang: Lang) -> anyhow::Error {
    let (new, old) = plan.split_at(done);
    if new.is_empty() {
        return anyhow!(
            tr(lang, Msg::PluginNothingReplaced).replace("{cause}", &format!("{cause:#}"))
        );
    }
    let old: Vec<String> = old
        .iter()
        .map(|s| {
            if *s == STEP_PROGRAM {
                tr(lang, Msg::PluginPreviousRunning)
                    .replace("{step}", tr(lang, Msg::PluginStepProgram))
            } else {
                step_text(s, lang)
            }
        })
        .collect();
    let repair = if new.contains(&STEP_LIST) {
        tr(lang, Msg::PluginRepairUpdate)
    } else {
        tr(lang, Msg::PluginRepairRetry)
    };
    anyhow!(tr_format(lang, Msg::PluginReplacementPartial, &[
        ("cause", &format!("{cause:#}")),
        ("new", &new.iter().map(|s| step_text(s, lang)).collect::<Vec<_>>().join(", ")),
        ("old", &old.join(", ")), ("repair", repair),
    ]))
}

fn run_checked(
    cargo: &Path,
    args: &[&str],
    dir: &Path,
    target: &Path,
    interrupted: &AtomicBool,
    lang: Lang,
) -> Result<()> {
    let st = run_cargo(cargo, args, dir, target, interrupted, lang)?;
    if !st.success() {
        bail!(tr_format(lang, Msg::PluginCargoFailed, &[("args", &args.join(" ")), ("status", &super::status_text(st, lang))]));
    }
    Ok(())
}

/// A copy of the built program next to its place, not yet the program. Dropped without
/// [`StagedProgram::commit`] it deletes itself.
struct StagedProgram {
    tmp: PathBuf,
    dest: PathBuf,
    lang: Lang,
}

impl StagedProgram {
    fn new_lang(data_dir: &Path, built: &Path, lang: Lang) -> Result<StagedProgram> {
        use std::os::unix::fs::PermissionsExt;
        let dest = switch::binary_path(data_dir);
        let dir = dest.parent().expect("bin dir");
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".konoma.new-{}", std::process::id()));
        // Owned from here, so a failed copy or chmod removes what it left.
        let staged = StagedProgram { tmp, dest, lang };
        std::fs::copy(built, &staged.tmp).with_context(|| {
            tr(lang, Msg::PluginCannotWrite).replace("{path}", &staged.tmp.display().to_string())
        })?;
        std::fs::set_permissions(&staged.tmp, std::fs::Permissions::from_mode(0o755))?;
        Ok(staged)
    }

    /// Rename it over the program.
    fn commit(self) -> Result<()> {
        std::fs::rename(&self.tmp, &self.dest).with_context(|| {
            tr(self.lang, Msg::PluginCannotReplace)
                .replace("{path}", &self.dest.display().to_string())
        })
    }
}

impl Drop for StagedProgram {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tmp);
    }
}

/// Remove the rebuilt program, its record and the saved lock: konoma is the standard one again.
/// The program goes first: it is the one that decides what starts, so even if a later removal
/// fails, the standard konoma already runs.
pub fn remove_program_lang(data_dir: &Path, lang: Lang) -> Result<()> {
    for (i, p) in [
        switch::binary_path(data_dir),
        switch::record_path(data_dir),
        data_dir.join("Cargo.lock"),
    ]
    .iter()
    .enumerate()
    {
        match std::fs::remove_file(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                let note = if i == 0 {
                    ""
                } else {
                    tr(lang, Msg::PluginProgramAlreadyRemoved)
                };
                return Err(e).with_context(|| {
                    format!(
                        "{}{note}",
                        tr(lang, Msg::PluginCannotRemove)
                            .replace("{path}", &p.display().to_string())
                    )
                });
            }
        }
    }
    Ok(())
}

/// Remove the kept build cache and any work directory a killed run left. Returns the paths removed.
pub fn clean_lang(data_dir: &Path, lang: Lang) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let rd = match std::fs::read_dir(data_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
        Err(e) => {
            return Err(e).with_context(|| {
                tr(lang, Msg::PluginCannotEnumerate)
                    .replace("{path}", &data_dir.display().to_string())
            })
        }
    };
    for e in rd {
        let e = e.with_context(|| {
            tr(lang, Msg::PluginCannotEnumerate).replace("{path}", &data_dir.display().to_string())
        })?;
        let name = e.file_name().to_string_lossy().into_owned();
        if (name == "build" || name.starts_with("build-")) && e.path().is_dir() {
            std::fs::remove_dir_all(e.path()).with_context(|| {
                tr(lang, Msg::PluginCannotRemove).replace("{path}", &e.path().display().to_string())
            })?;
            removed.push(e.path());
        }
    }
    removed.sort();
    Ok(removed)
}

fn step_text(step: &str, lang: Lang) -> String {
    match step {
        STEP_RECORD => tr(lang, Msg::PluginStepRecord).into(),
        STEP_PROGRAM => tr(lang, Msg::PluginStepProgram).into(),
        _ => step.into(),
    }
}
#[cfg(test)]
fn no_cargo_message() -> String {
    no_cargo_message_lang(Lang::En)
}
#[cfg(test)]
fn verify_with_timeout(
    bin: &Path,
    own: &str,
    interrupted: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    verify_with_timeout_lang(bin, own, interrupted, timeout, Lang::En)
}
#[cfg(test)]
fn clean(data: &Path) -> Result<Vec<PathBuf>> {
    clean_lang(data, Lang::En)
}
#[cfg(test)]
fn remove_program(data: &Path) -> Result<()> {
    remove_program_lang(data, Lang::En)
}
#[cfg(test)]
impl WorkDir {
    fn create(data: &Path, keep: bool) -> Result<Self> {
        Self::create_lang(data, keep, Lang::En)
    }
    fn create_with(
        data: &Path,
        keep: bool,
        create: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<Self> {
        Self::create_with_lang(data, keep, Lang::En, create)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn unfinished_substitution_preserves_the_original_cause() {
        for lang in [Lang::En, Lang::Jp] {
            let cause = anyhow::anyhow!("data {cause} {new} {old} {repair}");
            let error = unfinished_lang(&cause, &[STEP_LOCK, STEP_LIST, STEP_PROGRAM], 1, lang).to_string();
            assert!(error.contains("data {cause} {new} {old} {repair}"), "{error}");
        }
    }

    #[test]
    fn build_error_language_walk() {
        use super::*;
        use crate::test_support::plugin_language::assert_japanese_plugin_prose;
        for lang in [Lang::Jp, Lang::En] {
            let dir = crate::test_support::unique_tmp("build-error-language");
            std::fs::create_dir_all(&dir).unwrap();
            let mut env = env_in(&dir);
            env.lang = lang;
            let mut errors = vec![Interrupted(lang).to_string(), no_cargo_message_lang(lang)];
            let create = WorkDir::create_with_lang(&env.data_dir, false, lang, |_| {
                Err(std::io::Error::from_raw_os_error(libc::EACCES))
            });
            errors.push(format!("{:#}", create.err().unwrap()));
            let built = dir.join("built");
            crate::test_support::write_executable(&built, "#!/bin/sh\nexit 37\n");
            errors.push(format!(
                "{:#}",
                run_checked(&built, &[], &dir, &dir, &AtomicBool::new(false), lang).unwrap_err()
            ));
            errors.push(format!(
                "{:#}",
                verify_with_timeout_lang(
                    &dir.join("absent"),
                    "0.31.2",
                    &AtomicBool::new(false),
                    Duration::from_secs(10),
                    lang
                )
                .unwrap_err()
            ));
            let cause = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::EACCES));
            for done in 0..4 {
                errors.push(
                    unfinished_lang(
                        &cause,
                        &[STEP_LOCK, STEP_LIST, STEP_RECORD, STEP_PROGRAM],
                        done,
                        lang,
                    )
                    .to_string(),
                );
            }
            let record = Record {
                konoma_version: "0.31.2".into(),
                plugins: std::collections::BTreeMap::new(),
            };
            std::fs::create_dir_all(env.lock_path()).unwrap();
            errors.push(format!(
                "{:#}",
                commit(&env, &built, &record, Some("version = 4"), None).unwrap_err()
            ));
            std::fs::remove_dir(env.lock_path()).unwrap();
            errors.push(format!(
                "{:#}",
                StagedProgram::new_lang(&env.data_dir, &dir.join("absent"), lang)
                    .err()
                    .unwrap()
            ));
            for path in [
                switch::binary_path(&env.data_dir),
                switch::record_path(&env.data_dir),
                env.lock_path(),
            ] {
                std::fs::create_dir_all(&path).unwrap();
                errors.push(format!(
                    "{:#}",
                    remove_program_lang(&env.data_dir, lang).unwrap_err()
                ));
                std::fs::remove_dir(&path).unwrap();
            }
            let bad = dir.join("file");
            std::fs::write(&bad, b"x").unwrap();
            errors.push(format!("{:#}", clean_lang(&bad, lang).unwrap_err()));
            let unreadable = dir.join("unreadable");
            std::fs::create_dir(&unreadable).unwrap();
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::metadata(&unreadable).unwrap().permissions();
            struct Restore(std::path::PathBuf, std::fs::Permissions);
            impl Drop for Restore {
                fn drop(&mut self) {
                    std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
                }
            }
            let _restore = Restore(unreadable.clone(), permissions);
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o300)).unwrap();
            errors.push(format!("{:#}", clean_lang(&unreadable, lang).unwrap_err()));
            if lang == Lang::Jp {
                for error in errors { assert_japanese_plugin_prose("build failure", &error); }
            } else {
                // Own contexts copied from 7359773; OS causes remain platform data.
                let pid = std::process::id();
                let expected = [
                    "interrupted; the previous konoma is unchanged".into(),
                    "cargo was not found (set $CARGO, or put cargo in PATH).".into(),
                    format!("cannot create {}/data/build-{pid}", dir.display()),
                    "`cargo ` failed (exit status: 37); the previous konoma is unchanged".into(),
                    format!("the new build did not finish `--plugins`: plugin job watcher: cannot start watched program {}/absent", dir.display()),
                ];
                for (error, expected) in errors.iter().zip(expected) {
                    crate::test_support::plugin_english_lower::assert_english_context("build", error, &expected);
                }
                assert_eq!(errors[1], "cargo was not found (set $CARGO, or put cargo in PATH).\n`konoma plugin install` and `update` rebuild konoma with Cargo, so they need a Rust toolchain.\nWithout one, install the plugin as a standalone program (for example `konoma-office`) and let\nkonoma run it as a child process: in config.toml write\n\n    [plugins.<id>]\n    command = \"/path/to/the-program\"\n");
                let cause = std::io::Error::from_raw_os_error(libc::EACCES).to_string();
                let plan = ["Cargo.lock", "plugins.toml", "the record of the program", "the konoma program (the previous one keeps running)"];
                for done in 0..4 {
                    let expected = if done == 0 {
                        format!("{cause}\nNothing was replaced; the previous konoma is unchanged. Run the command again once that is fixed.")
                    } else {
                        let repair = if done >= 2 { "`konoma plugin update` builds konoma from plugins.toml as it is now and brings the rest in line" } else { "run the same command again once that is fixed" };
                        format!("{cause}\nThe replacement stopped half way.\n  Now new:  {}\n  Still old: {}\n{repair}.", plan[..done].join(", "), plan[done..].join(", "))
                    };
                    assert_eq!(errors[5 + done], expected);
                }
                let expected = [
                    format!("cannot replace {}/data/Cargo.lock", dir.display()),
                    format!("cannot write {}/data/bin/.konoma.new-{pid}", dir.display()),
                    format!("cannot remove {}/data/bin/konoma", dir.display()),
                    format!("cannot remove {}/data/bin/konoma.built.toml (the rebuilt program itself is already removed)", dir.display()),
                    format!("cannot remove {}/data/Cargo.lock (the rebuilt program itself is already removed)", dir.display()),
                    format!("cannot enumerate {}/file", dir.display()),
                    format!("cannot enumerate {}/unreadable", dir.display()),
                ];
                assert_eq!(errors.len(), 9 + expected.len());
                for (error, expected) in errors[9..].iter().zip(expected) {
                    crate::test_support::plugin_english_lower::assert_english_context("build file", error, &expected);
                }
            }
        }
    }
    #[test]
    fn metadata_exit_stops_helpers_holding_its_output_pipes() {
        let dir = crate::test_support::unique_tmp("metadata-pipe-holder");
        std::fs::create_dir_all(&dir).unwrap();
        let mut fixture = crate::test_support::ProgramFixture::new(&dir,
            "/bin/sleep 300 &\nprintf '%s\\n' \"$!\" >&3\nprintf '%s' '{\"packages\":[{\"id\":\"fixture\",\"name\":\"konoma-foo\",\"authors\":[\"Fixture author\"]}],\"resolve\":{\"root\":\"root\",\"nodes\":[{\"id\":\"root\",\"deps\":[{\"pkg\":\"fixture\"}]}]}}'\nexit 0");
        let mut env = env_in(&dir);
        env.cargo_env = Some(fixture.program.clone().into_os_string());
        let data = env.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(super::authors(
                &env,
                &super::PluginList::default(),
                "konoma-foo",
            ))
            .unwrap();
        });
        fixture.ready();
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        if result.is_err() {
            fixture.stop();
        }
        worker.join().unwrap();
        assert_eq!(
            result.expect("metadata output watchdog").unwrap(),
            ["Fixture author"]
        );
        assert!(fixture.gone());
        assert!(!std::fs::read_dir(data).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("build-")));
    }
    #[test]
    fn partially_created_work_directory_is_removed() {
        let dir = crate::test_support::unique_tmp("partial-work");
        std::fs::create_dir_all(&dir).unwrap();
        let mut parent = None;
        let result = super::WorkDir::create_with(&dir, false, |path| {
            let path = path.parent().unwrap();
            std::fs::create_dir_all(path)?;
            parent = Some(path.to_owned());
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert!(result.is_err());
        assert!(
            !parent.unwrap().exists(),
            "failed creation left the run's directory"
        );
    }
    #[test]
    fn successful_verification_entry_cleans_its_background_helper() {
        let dir = crate::test_support::unique_tmp("verify-entry-background");
        std::fs::create_dir_all(&dir).unwrap();
        let mut fixture = crate::test_support::ProgramFixture::new(
            &dir,
            "/bin/sleep 300 </dev/null >/dev/null 2>&1 &\nprintf '%s\\n' \"$!\" >&3\nexit 0",
        );
        let program = fixture.program.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(super::verify_with_timeout(
                &program,
                "fixture",
                &std::sync::atomic::AtomicBool::new(false),
                std::time::Duration::MAX,
            ))
            .unwrap();
        });
        fixture.ready();
        let result = rx.recv_timeout(std::time::Duration::from_secs(10));
        let gone = fixture.gone();
        worker.join().unwrap();
        result.unwrap().unwrap();
        assert!(
            gone,
            "successful verification left its background helper alive"
        );
    }
    use super::*;
    use konoma_test_support::{unique_tmp, TmpDir};
    use std::os::unix::fs::PermissionsExt;

    mod process_fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/process_fixture.rs"
        ));
    }

    #[test]
    fn verification_cleans_helpers_after_success_and_failure() {
        let mut leaked = Vec::new();
        for status in [0, 1] {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", &format!("exit {status}")])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            in_own_group(&mut command);
            let pending = process_fixture::PendingHelper::attach(&mut command, false, true);
            let mut child = command.spawn().unwrap();
            let mut helper = pending.ready();
            let output = collect_output(
                &mut child,
                &AtomicBool::new(false),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
            assert_eq!(output.status.code(), Some(status));
            if !helper.gone() {
                leaked.push(status);
            }
        }
        assert!(
            leaked.is_empty(),
            "verification left helpers alive for exit codes {leaked:?}"
        );
    }

    fn exe(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn readiness_pid(path: &Path, timeout: Duration) -> Result<u32> {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening both ends is immediate even when the child has died before opening its end.
        let mut fifo = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        loop {
            if Instant::now() >= deadline {
                bail!("readiness deadline exceeded");
            }
            let mut buf = [0; 64];
            match fifo.read(&mut buf) {
                Ok(n) if n > 0 => {
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
                        return Ok(std::str::from_utf8(&bytes[..end])?.trim().parse()?);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn verification_readiness_without_a_writer_has_a_deadline() {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let d = tmp("verify-unready");
        let ready = d.join("ready");
        let path = std::ffi::CString::new(ready.to_str().unwrap()).unwrap();
        // SAFETY: this test owns the FIFO and passes a valid C string.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let reader_path = ready.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            tx.send(readiness_pid(&reader_path, Duration::from_millis(100)))
                .unwrap();
        });
        let result = rx.recv_timeout(Duration::from_secs(2));
        if result.is_err() {
            // A read/write open never waits for a peer. Keep it alive until the old reader exits,
            // even if that thread was delayed before open; no failed mutation can hang this test.
            let mut rescue = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&ready)
                .unwrap();
            rescue.write_all(b"invalid\n").unwrap();
            reader.join().unwrap();
        } else {
            reader.join().unwrap();
        }
        let error = result
            .expect("readiness must have its own deadline")
            .unwrap_err();
        assert!(error.to_string().contains("deadline"), "{error:#}");
    }

    fn inherited_pipes(interrupt: bool) {
        inherited_pipes_with_deadline(interrupt, Arc::new(AtomicBool::new(false)), move || {
            if interrupt {
                Duration::from_secs(60)
            } else {
                Duration::from_millis(100)
            }
        });
    }

    fn inherited_pipes_with_deadline(
        interrupt: bool,
        ready_state: Arc<AtomicBool>,
        deadline: impl FnOnce() -> Duration + Send + 'static,
    ) {
        capture_pipe_fixture(interrupt, false, ready_state, deadline);
    }

    fn capture_pipe_fixture(
        interrupt: bool,
        detached: bool,
        ready_state: Arc<AtomicBool>,
        deadline: impl FnOnce() -> Duration + Send + 'static,
    ) {
        let mut command = Command::new("/bin/sh");
        command
            // A natural exit now kills same-group pipe holders immediately. Keep the producer
            // alive here to exercise the run deadline; detached holders still test post-exit I/O.
            .args([
                "-c",
                if detached {
                    "exit 0"
                } else {
                    "exec /bin/sleep 300"
                },
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        in_own_group(&mut command);
        let pending = process_fixture::PendingHelper::attach(&mut command, detached, false);
        let mut child = command.spawn().unwrap();
        let mut helper = pending.ready();
        // Startup and its readiness handshake are complete before the deadline is constructed.
        ready_state.store(true, Ordering::SeqCst);
        let timeout = deadline();
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(collect_output(&mut child, &worker_flag, Some(timeout)))
                .unwrap();
        });
        if interrupt {
            flag.store(true, Ordering::SeqCst);
        }
        let result = rx.recv_timeout(Duration::from_secs(6));
        if result.is_err() {
            helper.stop();
        }
        worker.join().unwrap();
        helper.stop();
        assert!(helper.gone());
        let error = result
            .expect("pipe collection must obey its bound")
            .unwrap_err();
        if interrupt {
            assert!(error.is::<Interrupted>(), "{error:#}");
        } else {
            assert!(error.to_string().contains("timed out"), "{error:#}");
        }
    }

    #[test]
    fn verification_fixture_starts_its_deadline_only_after_readiness() {
        let ready = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let seen = observed.clone();
        let ready_at_start = ready.clone();
        inherited_pipes_with_deadline(false, ready, move || {
            seen.store(ready_at_start.load(Ordering::SeqCst), Ordering::SeqCst);
            Duration::from_millis(100)
        });
        assert!(
            observed.load(Ordering::SeqCst),
            "the timeout was started before the child was ready"
        );
    }

    #[test]
    fn verification_deadline_includes_inherited_pipes() {
        inherited_pipes(false);
    }

    fn detached_pipes(interrupt: bool) {
        capture_pipe_fixture(
            interrupt,
            true,
            Arc::new(AtomicBool::new(false)),
            move || {
                if interrupt {
                    Duration::from_secs(60)
                } else {
                    Duration::from_millis(100)
                }
            },
        );
    }

    #[test]
    fn verification_deadline_includes_detached_pipe_holders() {
        detached_pipes(false);
    }

    #[test]
    fn verification_interruption_includes_detached_pipe_holders() {
        detached_pipes(true);
    }

    #[test]
    fn verification_interruption_includes_inherited_pipes() {
        inherited_pipes(true);
    }

    #[test]
    fn stopping_a_live_group_allows_its_term_handler_to_finish() {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        use std::os::{fd::OwnedFd, unix::net::UnixStream};
        let leader = in_own_group(&mut Command::new("/bin/cat"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut leader = konoma_plugin::proc::process::ChildGuard::new(leader);
        let pgid = leader.id();
        let (output, child_output) = UnixStream::pair().unwrap();
        output
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let member = Command::new("/bin/sh")
            .args([
                "-c",
                "trap 'echo term; read line; echo graceful; exit 0' TERM; echo ready; read line",
            ])
            .process_group(pgid as i32)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(OwnedFd::from(child_output)))
            .spawn()
            .unwrap();
        let mut member = konoma_plugin::proc::process::ChildGuard::new(member);
        let mut output = std::io::BufReader::new(output);
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        // The leader remains alive on its open stdin until stop_group sends TERM.
        assert!(konoma_plugin::proc::process::poll_exit(&leader)
            .unwrap()
            .is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel::<()>();
        let (permit_tx, permit_rx) = std::sync::mpsc::channel();
        let stopper = std::thread::spawn(move || {
            let mut first = true;
            konoma_plugin::proc::process::stop_group_with_wait_hook(
                &mut leader,
                Duration::MAX,
                || {
                    if first {
                        first = false;
                        waiting_tx.send(()).unwrap();
                        permit_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                    }
                },
            )
            .unwrap();
            tx.send(()).unwrap();
        });
        line.clear();
        output.read_line(&mut line).unwrap();
        // A member of the live group must be given time to finish its TERM handler.
        let waiting = waiting_rx.recv_timeout(Duration::from_secs(5));
        use std::io::Write;
        let write = member.stdin.as_mut().unwrap().write_all(b"done\n");
        let status =
            konoma_plugin::proc::process::wait(&mut member, Duration::from_secs(10)).unwrap();
        let _ = permit_tx.send(());
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stopper.join().unwrap();
        waiting.expect("the fixture must hold the live-member grace before releasing the handler");
        assert_eq!(line, "term\n");
        write.unwrap();
        assert!(
            status.success(),
            "the member was killed before completing its TERM handler: {status}"
        );
    }

    fn tmp(p: &str) -> TmpDir {
        let d = unique_tmp(p);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cargo_comes_from_the_env_var_first() {
        let d = tmp("fc-env");
        exe(&d.join("mine").join("cargo"));
        exe(&d.join("bin").join("cargo"));
        let path = d.join("bin");
        let got = find_cargo(
            Some(d.join("mine").join("cargo").as_os_str()),
            Some(path.as_os_str()),
        );
        assert_eq!(got, Some(d.join("mine").join("cargo")));
    }

    #[test]
    fn cargo_in_path_when_the_var_is_unset_or_empty() {
        let d = tmp("fc-path");
        exe(&d.join("bin").join("cargo"));
        let path = d.join("bin");
        assert_eq!(
            find_cargo(None, Some(path.as_os_str())),
            Some(d.join("bin").join("cargo"))
        );
        assert_eq!(
            find_cargo(Some(OsStr::new("")), Some(path.as_os_str())),
            Some(d.join("bin").join("cargo"))
        );
        // A bare name in the variable is looked up too.
        assert_eq!(
            find_cargo(Some(OsStr::new("cargo")), Some(path.as_os_str())),
            Some(d.join("bin").join("cargo"))
        );
    }

    #[test]
    fn no_cargo_is_none_and_the_message_names_the_other_way() {
        let d = tmp("fc-none");
        let path = d.join("empty");
        assert_eq!(find_cargo(None, Some(path.as_os_str())), None);
        assert_eq!(find_cargo(None, None), None);
        // The variable names a file that is not there: no fallback to PATH.
        exe(&d.join("bin").join("cargo"));
        let p2 = d.join("bin");
        assert_eq!(
            find_cargo(Some(d.join("nope").as_os_str()), Some(p2.as_os_str())),
            None
        );
        let m = no_cargo_message();
        assert!(m.contains("cargo was not found"));
        assert!(m.contains("[plugins.<id>]") && m.contains("command"));
        assert!(m.contains("child process"));
    }

    #[test]
    fn a_file_in_path_that_is_not_executable_is_not_cargo() {
        let d = tmp("fc-nonexec");
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::write(d.join("bin").join("cargo"), "x").unwrap();
        let path = d.join("bin");
        assert_eq!(find_cargo(None, Some(path.as_os_str())), None);
    }

    #[test]
    fn clean_propagates_unreadable_directory_errors() {
        let d = tmp("clean-unreadable");
        let original = std::fs::metadata(&d).unwrap().permissions();
        struct Restore<'a>(&'a Path, std::fs::Permissions);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                std::fs::set_permissions(self.0, self.1.clone()).unwrap();
            }
        }
        let _restore = Restore(&d, original);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o300)).unwrap();
        let error = clean(&d).expect_err("an unreadable directory must not look empty");
        assert!(
            error.chain().any(|e| e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)),
            "{error:#}"
        );
    }

    #[test]
    fn clean_removes_the_cache_and_leftover_work_dirs_only() {
        let d = tmp("clean");
        for n in ["build", "build-123", "bin", "other"] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        std::fs::write(d.join("Cargo.lock"), "x").unwrap();
        std::fs::write(d.join("build-file"), "x").unwrap();
        let removed = clean(&d).unwrap();
        assert_eq!(removed, [d.join("build"), d.join("build-123")]);
        assert!(d.join("bin").is_dir() && d.join("other").is_dir());
        assert!(d.join("Cargo.lock").is_file() && d.join("build-file").is_file());
        assert!(clean(&d).unwrap().is_empty());
        assert!(clean(&d.join("missing")).unwrap().is_empty());
    }

    #[test]
    fn remove_program_deletes_the_three_files_and_tolerates_absence() {
        let d = tmp("rmprog");
        exe(&switch::binary_path(&d));
        Record {
            konoma_version: "1.0.0".into(),
            plugins: std::collections::BTreeMap::new(),
        }
        .save(&d)
        .unwrap();
        std::fs::write(d.join("Cargo.lock"), "x").unwrap();
        remove_program(&d).unwrap();
        assert!(!switch::binary_path(&d).exists());
        assert!(!switch::record_path(&d).exists());
        assert!(!d.join("Cargo.lock").exists());
        remove_program(&d).unwrap();
    }

    #[test]
    fn the_work_dir_is_removed_on_drop_unless_kept() {
        let d = tmp("workdir");
        let p = {
            let w = WorkDir::create(&d, false).unwrap();
            assert!(w.path.join("src").is_dir());
            w.path.clone()
        };
        assert!(!p.exists());
        let kept = {
            let w = WorkDir::create(&d, true).unwrap();
            w.path.clone()
        };
        assert!(kept.is_dir());
        assert_eq!(kept, d.join("build"));
    }

    fn env_in(d: &TmpDir) -> Env {
        Env {
            config_dir: d.join("config"),
            data_dir: d.join("data"),
            home: None,
            cargo_env: None,
            path_env: None,
            konoma_version: "0.31.2".into(),
            lang: crate::i18n::Lang::En,
            stdin_is_tty: false,
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn keep_build_cache_reads_the_config() {
        let d = tmp("keep");
        let env = env_in(&d);
        assert!(!env.keep_build_cache(), "no config file");
        std::fs::create_dir_all(&env.config_dir).unwrap();
        let cfg = env.config_dir.join("config.toml");
        for (text, want) in [
            ("[plugins]\nkeep_build_cache = true\n", true),
            ("[plugins]\nkeep_build_cache = false\n", false),
            ("[plugins]\nkeep_build_cache = \"yes\"\n", false),
            ("[plugins.markdown]\nalerts = true\n", false),
            (
                "[plugins]\nkeep_build_cache = true\n[plugins.markdown]\nalerts = true\n",
                true,
            ),
            ("= broken", false),
        ] {
            std::fs::write(&cfg, text).unwrap();
            assert_eq!(env.keep_build_cache(), want, "{text}");
        }
    }

    #[test]
    fn rebuild_without_cargo_says_so_before_touching_anything() {
        let d = tmp("rebuild-nocargo");
        let env = env_in(&d);
        let e = rebuild(&env, &PluginList::default(), Update::Nothing, true).unwrap_err();
        assert!(e.to_string().contains("cargo was not found"), "{e}");
        assert!(!env.data_dir.exists(), "nothing was created");
        assert!(!env.plugins_toml().exists());
    }
}
