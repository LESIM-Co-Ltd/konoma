//! The konoma rebuilt with plugins, where it lives, and how a starting konoma hands over to it.
//!
//! `konoma plugin install` puts the rebuilt program at `<data>/bin/konoma` and, next to it, a small
//! record (`konoma.built.toml`) of the konoma version it was built from and the plugin crates in
//! it. A konoma that starts and finds both -- and is not that program itself -- replaces itself
//! with it (`exec`: same arguments, same environment). Two things keep this from looping: the
//! program is only chosen when it is a different file from the running one, and the hand-over sets
//! [`SWITCHED_ENV`], which a konoma that finds it set never hands over again. The variable also
//! carries the version of the konoma that handed over, so the rebuilt program can tell it is older
//! than the konoma the user installed.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::list_file::write_atomic_lang;
use crate::i18n::Lang;

/// Set by a konoma that handed over; its value is that konoma's version.
pub const SWITCHED_ENV: &str = "KONOMA_PLUGIN_SWITCHED_FROM";
/// Original executable path, kept separate so the existing version marker retains its format.
pub const SWITCHED_PATH_ENV: &str = "KONOMA_PLUGIN_SWITCHED_PATH";

fn original_exe(from: Option<&OsStr>, current: Option<PathBuf>) -> Option<PathBuf> {
    from.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or(current)
}

/// Consume the path before the version marker and before any threads or child processes exist.
pub fn take_original_exe() -> Option<PathBuf> {
    let from = std::env::var_os(SWITCHED_PATH_ENV);
    if from.is_some() {
        debug_assert!(thread_count().is_none_or(|n| n == 1));
        std::env::remove_var(SWITCHED_PATH_ENV);
    }
    original_exe(from.as_deref(), std::env::current_exe().ok())
}

/// Read [`SWITCHED_ENV`] and take it out of the environment, so that nothing konoma starts later
/// (the shell or editor opened from it, a delegated command, the drawing child, a konoma started
/// from those) inherits it: it would make that konoma believe it was handed over to, and skip
/// its own hand-over and show the "older than" notice.
///
/// Must run while the process is still single-threaded: changing the environment while another
/// thread reads it is a data race (`remove_var` is `unsafe` from edition 2024 for that reason).
/// `Host::builder` calls it before any plugin factories or manifests run.
pub fn take_switched_from() -> Option<std::ffi::OsString> {
    let value = std::env::var_os(SWITCHED_ENV);
    if value.is_some() {
        // The generated main creates its builder before evaluating any plugin factory.
        debug_assert!(
            thread_count().is_none_or(|n| n == 1),
            "a thread was started before the environment was changed ({:?} threads); create the Host builder before starting threads",
            thread_count()
        );
        std::env::remove_var(SWITCHED_ENV);
    }
    value
}

/// How many threads this process has now, where the system can say (Linux, macOS).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn thread_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        Some(std::fs::read_dir("/proc/self/task").ok()?.count())
    }
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: proc_pidinfo writes at most `size` bytes into `info`, a zeroed buffer of exactly
        // that size; `info` is only read when the call reports that it filled all of it.
        let got = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        (got == size).then(|| unsafe { info.assume_init() }.pti_threadnum.max(0) as usize)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn thread_count() -> Option<usize> {
    None
}

/// The rebuilt program.
pub fn binary_path(data_dir: &Path) -> PathBuf {
    data_dir.join("bin").join("konoma")
}

/// The record next to it.
pub fn record_path(data_dir: &Path) -> PathBuf {
    data_dir.join("bin").join("konoma.built.toml")
}

/// What the record says about the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The konoma version the program was built from.
    pub konoma_version: String,
    /// Resolved versions of the plugin crates, including the standard plugins.
    pub plugins: BTreeMap<String, String>,
}

impl Record {
    pub fn to_toml(&self) -> String {
        let mut t = toml::Table::new();
        t.insert("konoma_version".into(), self.konoma_version.clone().into());
        t.insert(
            "plugins".into(),
            toml::Value::Table(
                self.plugins
                    .iter()
                    .map(|(n, v)| (n.clone(), v.clone().into()))
                    .collect(),
            ),
        );
        toml::to_string(&t).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Record> {
        let t: toml::Table = text.parse().ok()?;
        let konoma_version = t.get("konoma_version")?.as_str()?.to_string();
        let plugins = t
            .get("plugins")?
            .as_table()?
            .iter()
            .map(|(n, v)| v.as_str().map(|v| (n.clone(), v.to_owned())))
            .collect::<Option<BTreeMap<_, _>>>()?;
        Some(Record {
            konoma_version,
            plugins,
        })
    }

    pub fn save_lang(&self, data_dir: &Path, lang: Lang) -> Result<()> {
        write_atomic_lang(&record_path(data_dir), self.to_toml().as_bytes(), lang)
    }
}

#[cfg(test)]
impl Record {
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        self.save_lang(data_dir, Lang::En)
    }
}

/// The record of the program in `data_dir`, when there is one *and* the program exists.
pub fn read_record(data_dir: &Path) -> Option<Record> {
    if !binary_path(data_dir).is_file() {
        return None;
    }
    Record::parse(&std::fs::read_to_string(record_path(data_dir)).ok()?)
}

pub fn legacy_notice(data_dir: &Path, lang: crate::i18n::Lang) -> Option<String> {
    if !binary_path(data_dir).is_file() {
        return None;
    }
    let record: toml::Table = std::fs::read_to_string(record_path(data_dir))
        .ok()?
        .parse()
        .ok()?;
    record.get("plugins")?.as_array()?;
    Some(crate::i18n::tr(lang, crate::i18n::Msg::PluginBuildLegacy).to_owned())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether the running file is the rebuilt program.
pub fn is_rebuilt_program(current_exe: &Path, data_dir: &Path) -> bool {
    same_file(current_exe, &binary_path(data_dir))
}

/// The program to hand over to, if this start should: `args` are the process arguments
/// (`args[0]` is the program name). No hand-over when the command is `konoma plugin ...` (it must
/// work with a broken rebuilt program, so that `remove` and `update` can repair it), when
/// [`SWITCHED_ENV`] is set, when there is no rebuilt program with its record, or when the running
/// file already is that program.
pub fn target(
    args: &[String],
    switched_env: Option<&OsStr>,
    current_exe: Option<&Path>,
    data_dir: Option<&Path>,
) -> Option<PathBuf> {
    if args.get(1).is_some_and(|a| a == "plugin") {
        return None;
    }
    if switched_env.is_some() {
        return None;
    }
    let data_dir = data_dir?;
    read_record(data_dir)?;
    let bin = binary_path(data_dir);
    if let Some(cur) = current_exe {
        if same_file(cur, &bin) {
            return None;
        }
    }
    Some(bin)
}

/// `(major, minor, patch)` of a version such as `0.31.2` or `1.0.0-rc.1` (a pre-release suffix is
/// ignored). `None` when it does not look like one.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// Whether the program built from `own` is older than the konoma `from` that handed over to it.
/// `false` when either does not parse (no notice is better than a wrong one).
pub fn is_stale(own: &str, from: &str) -> bool {
    match (parse_version(own), parse_version(from)) {
        (Some(o), Some(f)) => o < f,
        _ => false,
    }
}

/// The notice for the start-up message, when the program running was handed over to by a newer
/// konoma. `from` is the value of [`SWITCHED_ENV`].
pub fn stale_notice(own: &str, from: Option<&OsStr>, lang: crate::i18n::Lang) -> Option<String> {
    let from = from?.to_str()?;
    if !is_stale(own, from) {
        return None;
    }
    Some(crate::i18n::tr_format(
        lang,
        crate::i18n::Msg::PluginBuildStale,
        &[("embedded", own), ("base", from)],
    ))
}

/// The `--version` text: `konoma 0.31.2`, or `konoma 0.31.2 (plugins: konoma-foo, konoma-bar)`
/// when the running file is the rebuilt program with plugins in it.
pub fn version_line(own: &str, current_exe: Option<&Path>, data_dir: Option<&Path>) -> String {
    let rebuilt = match (current_exe, data_dir) {
        (Some(exe), Some(dir)) if is_rebuilt_program(exe, dir) => read_record(dir),
        _ => None,
    };
    match rebuilt {
        Some(r) if !r.plugins.is_empty() => {
            format!(
                "konoma {own} (plugins: {})\n",
                r.plugins.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        }
        Some(_) => format!("konoma {own} (rebuilt)\n"),
        None => format!("konoma {own}\n"),
    }
}

/// Replace this process with `bin`, passing the arguments after `args[0]` and the environment on,
/// with [`SWITCHED_ENV`] set to `own`. Returns only when the hand-over failed.
pub fn exec(bin: &Path, args: &[String], own: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    handover_command(bin, args, own, std::env::current_exe().ok().as_deref()).exec()
}

fn handover_command(
    bin: &Path,
    args: &[String],
    own: &str,
    original: Option<&Path>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args.iter().skip(1)).env(SWITCHED_ENV, own);
    if let Some(original) = original {
        cmd.env(SWITCHED_PATH_ENV, original);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::{unique_tmp, TmpDir};

    #[test]
    fn update_handover_preserves_original_executable_separately_from_version() {
        let cmd = handover_command(
            Path::new("/data/bin/konoma"),
            &a(&["konoma", "."]),
            "0.31.2",
            Some(Path::new("/opt/homebrew/bin/konoma")),
        );
        let vars: std::collections::BTreeMap<_, _> = cmd.get_envs().collect();
        assert_eq!(vars[OsStr::new(SWITCHED_ENV)], Some(OsStr::new("0.31.2")));
        assert_eq!(
            vars[OsStr::new(SWITCHED_PATH_ENV)],
            Some(OsStr::new("/opt/homebrew/bin/konoma"))
        );
        assert_eq!(
            original_exe(
                Some(OsStr::new("/usr/bin/konoma")),
                Some(PathBuf::from("/data/bin/konoma"))
            ),
            Some(PathBuf::from("/usr/bin/konoma"))
        );
        assert_eq!(
            original_exe(None, Some(PathBuf::from("/example/.cargo/bin/konoma"))),
            Some(PathBuf::from("/example/.cargo/bin/konoma"))
        );
        assert_eq!(original_exe(None, None), None);
    }

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_thread_count_is_read_and_sees_a_thread_that_is_started() {
        let before = thread_count().expect("this system can say");
        assert!(before >= 1, "{before}");
        let (stop, parked) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            let _ = parked.recv();
        });
        let during = thread_count().unwrap();
        // At least the thread running the test and the one just started.
        assert!(during >= 2, "{during}");
        drop(stop);
        t.join().unwrap();
    }

    /// A data directory with (or without) a rebuilt program and its record.
    fn data(program: bool, record: bool) -> TmpDir {
        let d = unique_tmp("switch-data");
        std::fs::create_dir_all(d.join("bin")).unwrap();
        if program {
            std::fs::write(binary_path(&d), "#!/bin/sh\n").unwrap();
        }
        if record {
            Record {
                konoma_version: "0.31.2".into(),
                plugins: [("konoma-foo".into(), "0.3.2".into())].into(),
            }
            .save(&d)
            .unwrap();
        }
        d
    }

    #[test]
    fn hands_over_when_there_is_a_program_and_its_record() {
        let d = data(true, true);
        let t = target(
            &a(&["konoma", "x"]),
            None,
            Some(Path::new("/usr/bin/konoma")),
            Some(&d),
        );
        assert_eq!(t, Some(binary_path(&d)));
    }

    #[test]
    fn no_hand_over_without_the_program_or_the_record() {
        let exe = Path::new("/usr/bin/konoma");
        let args = a(&["konoma"]);
        assert_eq!(
            target(&args, None, Some(exe), Some(&data(false, true))),
            None
        );
        assert_eq!(
            target(&args, None, Some(exe), Some(&data(true, false))),
            None
        );
        assert_eq!(
            target(&args, None, Some(exe), Some(&data(false, false))),
            None
        );
        assert_eq!(target(&args, None, Some(exe), None), None);
        let missing = unique_tmp("switch-missing");
        assert_eq!(target(&args, None, Some(exe), Some(&missing)), None);
    }

    #[test]
    fn a_broken_record_means_no_hand_over() {
        let d = data(true, false);
        std::fs::write(record_path(&d), "not = = toml").unwrap();
        assert_eq!(target(&a(&["konoma"]), None, None, Some(&d)), None);
        std::fs::write(record_path(&d), "konoma_version = 3\n").unwrap();
        assert_eq!(target(&a(&["konoma"]), None, None, Some(&d)), None);
    }

    #[test]
    fn no_hand_over_when_the_running_file_is_the_program() {
        let d = data(true, true);
        assert_eq!(
            target(&a(&["konoma"]), None, Some(&binary_path(&d)), Some(&d)),
            None
        );
        // Through a symlink to it as well (the comparison is of the files, not the spellings).
        let link = d.join("link-to-konoma");
        std::os::unix::fs::symlink(binary_path(&d), &link).unwrap();
        assert_eq!(target(&a(&["konoma"]), None, Some(&link), Some(&d)), None);
        assert!(is_rebuilt_program(&link, &d));
        assert!(!is_rebuilt_program(Path::new("/usr/bin/konoma"), &d));
    }

    #[test]
    fn the_loop_guard_variable_stops_it() {
        let d = data(true, true);
        let t = target(
            &a(&["konoma"]),
            Some(OsStr::new("0.31.2")),
            Some(Path::new("/usr/bin/konoma")),
            Some(&d),
        );
        assert_eq!(t, None);
        // An empty value still counts as set.
        assert_eq!(
            target(&a(&["konoma"]), Some(OsStr::new("")), None, Some(&d)),
            None
        );
    }

    #[test]
    fn the_plugin_command_never_hands_over() {
        let d = data(true, true);
        for args in [
            a(&["konoma", "plugin"]),
            a(&["konoma", "plugin", "remove", "konoma-foo"]),
            a(&["konoma", "plugin", "update"]),
        ] {
            assert_eq!(
                target(&args, None, Some(Path::new("/usr/bin/konoma")), Some(&d)),
                None
            );
        }
        // `./plugin` (a folder named plugin) and later arguments named plugin do hand over.
        for args in [a(&["konoma", "./plugin"]), a(&["konoma", "x", "plugin"])] {
            assert!(target(&args, None, Some(Path::new("/usr/bin/konoma")), Some(&d)).is_some());
        }
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert_eq!(parse_version("0.31.2"), Some((0, 31, 2)));
        assert_eq!(parse_version("1.0.0-rc.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("1.0"), None);
        assert_eq!(parse_version("1.0.0.1"), None);
        assert_eq!(parse_version("x.y.z"), None);
        assert!(is_stale("0.9.0", "0.10.0"), "9 < 10 numerically");
        assert!(is_stale("0.31.2", "0.32.0"));
        assert!(!is_stale("0.32.0", "0.32.0"));
        assert!(!is_stale("0.33.0", "0.32.9"));
        assert!(!is_stale("garbage", "0.32.0"));
        assert!(!is_stale("0.32.0", "garbage"));
    }

    #[test]
    fn the_stale_notice_names_both_versions_in_both_languages() {
        use crate::i18n::Lang;
        for lang in [Lang::En, Lang::Jp] {
            let n = stale_notice("0.31.2", Some(OsStr::new("0.32.0")), lang).unwrap();
            assert!(n.contains("0.31.2") && n.contains("0.32.0"), "{n}");
            assert!(n.contains("konoma plugin update"), "{n}");
            assert!(!n.contains("{embedded}") && !n.contains("{base}"), "{n}");
        }
        let en = stale_notice("0.31.2", Some(OsStr::new("0.32.0")), Lang::En).unwrap();
        let jp = stale_notice("0.31.2", Some(OsStr::new("0.32.0")), Lang::Jp).unwrap();
        assert_ne!(en, jp);
        assert_eq!(
            stale_notice("0.32.0", Some(OsStr::new("0.32.0")), Lang::En),
            None
        );
        assert_eq!(
            stale_notice("0.33.0", Some(OsStr::new("0.32.0")), Lang::En),
            None
        );
        assert_eq!(stale_notice("0.31.2", None, Lang::En), None);
    }

    #[test]
    fn the_version_line_names_the_plugins_only_for_the_rebuilt_program() {
        let d = data(true, true);
        assert_eq!(
            version_line("0.31.2", Some(&binary_path(&d)), Some(&d)),
            "konoma 0.31.2 (plugins: konoma-foo)\n"
        );
        assert_eq!(
            version_line("0.31.2", Some(Path::new("/usr/bin/konoma")), Some(&d)),
            "konoma 0.31.2\n"
        );
        assert_eq!(version_line("0.31.2", None, None), "konoma 0.31.2\n");
        let no_record = data(true, false);
        assert_eq!(
            version_line("0.31.2", Some(&binary_path(&no_record)), Some(&no_record)),
            "konoma 0.31.2\n"
        );
    }

    #[test]
    fn a_legacy_record_is_obsolete_without_panicking() {
        let d = data(true, false);
        std::fs::write(
            record_path(&d),
            "konoma_version = \"0.31.2\"\nplugins = [\"old-plugin\"]\n",
        )
        .unwrap();
        assert!(read_record(&d).is_none());
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
            assert!(legacy_notice(&d, lang)
                .unwrap()
                .contains("konoma plugin update"));
        }
    }

    #[test]
    fn the_record_round_trips() {
        let r = Record {
            konoma_version: "0.31.2".into(),
            plugins: [("a".into(), "0.1.0".into()), ("b-c".into(), "0.2.0".into())].into(),
        };
        assert_eq!(Record::parse(&r.to_toml()), Some(r));
        let empty = Record {
            konoma_version: "1.0.0".into(),
            plugins: BTreeMap::new(),
        };
        assert_eq!(Record::parse(&empty.to_toml()), Some(empty));
        assert_eq!(
            Record::parse("plugins = [1]\nkonoma_version = \"1\"\n"),
            None
        );
    }
}
