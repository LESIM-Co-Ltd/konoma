//! `konoma plugin install | update | remove | list | clean`: konoma rebuilt with the plugin crates
//! of `plugins.toml` linked in (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §4-11). Plain standard
//! output and standard error; no terminal UI.
//!
//! A first argument of `plugin` is always this command (as with `git` and `cargo`); a folder of
//! that name opens with `konoma ./plugin`.

mod build;
mod list_file;
mod lock;
mod project;
pub(crate) mod switch;
pub(crate) mod update_check;
pub(crate) mod watcher;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::i18n::{tr, tr_format, Lang, Msg};
use anyhow::{bail, Context, Result};

use build::{Env, Interrupted, Update};
use list_file::{PluginList, Source};

/// Language for the early HostBuilder validation in the verified executable. It cannot read
/// configuration before validating manifests, so the plugin CLI supplies its resolved language.
pub(crate) const VERIFICATION_LANG_ENV: &str = "KONOMA_PLUGIN_VERIFY_LANG";
pub(crate) fn verification_language() -> Lang {
    if std::env::var_os(VERIFICATION_LANG_ENV).is_some_and(|value| value == "jp") {
        Lang::Jp
    } else {
        Lang::En
    }
}

/// Format only konoma-owned process errors; preserve OS diagnostics and Cargo's output.
fn process_error(error: std::io::Error, lang: Lang) -> anyhow::Error {
    if lang == Lang::En || error.raw_os_error().is_some() {
        return error.into();
    }
    let msg = if error.kind() == std::io::ErrorKind::TimedOut {
        // The SDK returns io::Error without a typed diagnostic kind, so this shared constant is the contract.
        if error.to_string() == konoma_plugin::proc::process::OUTPUT_PIPES_REMAINED_OPEN {
            Msg::PluginOutputOpen
        } else {
            Msg::PluginTimedOut
        }
    } else {
        Msg::PluginProcessIo
    };
    anyhow::anyhow!(tr(lang, msg).replace("{kind}", &format!("{:?}", error.kind())))
}

fn status_text(status: std::process::ExitStatus, lang: Lang) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        tr(lang, Msg::PluginExitCode).replace("{code}", &code.to_string())
    } else {
        tr_format(
            lang,
            Msg::PluginSignalStatus,
            &[
                ("status", &status.to_string()),
                ("signal", &status.signal().unwrap_or_default().to_string()),
            ],
        )
    }
}

/// Whether the process arguments (`args[0]` is the program) ask for this command.
pub fn is_plugin_command(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == "plugin")
}

#[cfg(test)]
pub(crate) fn generated_main_for_startup_test() -> String {
    let mut list = PluginList::default();
    list.plugins.insert(
        "noisy".into(),
        Source::Crates {
            version: "0.1".into(),
        },
    );
    project::generate(&list, env!("CARGO_PKG_VERSION"), None)
        .unwrap()
        .main_rs
}

/// A parsed `konoma plugin ...` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Install {
        name: String,
        source: Source,
        yes: bool,
    },
    Update(Option<String>),
    Remove(String),
    List,
    Clean,
    Help,
}

/// The usage text; `plugin_entry_here` adds the hint for a folder or file named `plugin`.
pub fn usage(plugin_entry_here: bool) -> String {
    // Keep usage English, consistently with the configuration-free main --help.
    let mut s = tr(Lang::En, Msg::PluginUsage).to_owned();
    if plugin_entry_here {
        s.push_str(tr(Lang::En, Msg::PluginFolderHint));
    }
    s
}

/// Parse the arguments after `plugin`. `Err` is the complaint to print above the usage.
pub fn parse_lang(args: &[String], lang: Lang) -> Result<Cmd, String> {
    let Some(sub) = args.first() else {
        return Err(tr(lang, Msg::PluginCommandNeeded).into());
    };
    let rest = &args[1..];
    match sub.as_str() {
        "help" | "--help" | "-h" => Ok(Cmd::Help),
        "list" => no_args(rest, "list", lang).map(|_| Cmd::List),
        "clean" => no_args(rest, "clean", lang).map(|_| Cmd::Clean),
        "remove" => match rest {
            [n] if list_file::valid_crate_name(n) => Ok(Cmd::Remove(n.clone())),
            [n] => Err(tr(lang, Msg::PluginBadCrate).replace("{name}", n)),
            [] => Err(tr(lang, Msg::PluginRemoveNeeds).into()),
            _ => Err(tr(lang, Msg::PluginRemoveOne).into()),
        },
        "update" => match rest {
            [] => Ok(Cmd::Update(None)),
            [n] if list_file::valid_crate_name(n) => Ok(Cmd::Update(Some(n.clone()))),
            [n] => Err(tr(lang, Msg::PluginBadCrate).replace("{name}", n)),
            _ => Err(tr(lang, Msg::PluginUpdateOneArg).into()),
        },
        "install" => parse_install(rest, lang),
        other => Err(tr(lang, Msg::PluginUnknownCommand).replace("{command}", other)),
    }
}

fn no_args(rest: &[String], cmd: &str, lang: Lang) -> Result<(), String> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(tr(lang, Msg::PluginNoArgs).replace("{command}", cmd))
    }
}

fn parse_install(rest: &[String], lang: Lang) -> Result<Cmd, String> {
    let (mut spec, mut git, mut branch, mut rev, mut path) = (None, None, None, None, None);
    let mut yes = false;
    let mut i = 0;
    let value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        rest.get(*i)
            .filter(|v| !v.starts_with("--") && !v.is_empty())
            .cloned()
            .ok_or_else(|| tr(lang, Msg::PluginFlagValue).replace("{flag}", flag))
    };
    while i < rest.len() {
        match rest[i].as_str() {
            "-y" | "--yes" => yes = true,
            "--git" => git = Some(value(&mut i, "--git")?),
            "--branch" => branch = Some(value(&mut i, "--branch")?),
            "--rev" => rev = Some(value(&mut i, "--rev")?),
            "--path" => path = Some(value(&mut i, "--path")?),
            a if a.starts_with('-') => {
                return Err(tr(lang, Msg::PluginUnknownOption).replace("{option}", a))
            }
            a => {
                if spec.replace(a.to_string()).is_some() {
                    return Err(tr(lang, Msg::PluginInstallOne).into());
                }
            }
        }
        i += 1;
    }
    let spec = spec.ok_or_else(|| tr(lang, Msg::PluginInstallNeeds).to_owned())?;
    let (name, version) = match spec.split_once('@') {
        Some((n, v)) if !v.is_empty() => (n.to_string(), Some(v.to_string())),
        Some(_) => return Err(tr(lang, Msg::PluginVersionNeeded).into()),
        None => (spec, None),
    };
    if !list_file::valid_crate_name(&name) {
        return Err(tr(lang, Msg::PluginBadCrate).replace("{name}", &name));
    }
    if git.is_some() && path.is_some() {
        return Err(tr(lang, Msg::PluginGitPath).into());
    }
    if (git.is_some() || path.is_some()) && version.is_some() {
        return Err(tr(lang, Msg::PluginVersionSource).into());
    }
    if git.is_none() && (branch.is_some() || rev.is_some()) {
        return Err(tr(lang, Msg::PluginGitSelector).into());
    }
    if branch.is_some() && rev.is_some() {
        return Err(tr(lang, Msg::PluginGitSelectorOne).into());
    }
    let source = if let Some(url) = git {
        Source::Git { url, branch, rev }
    } else if let Some(path) = path {
        // Made absolute against the current directory by the caller (`absolutize`).
        Source::Path { path }
    } else {
        Source::Crates {
            version: version.unwrap_or_else(|| "*".into()),
        }
    };
    Ok(Cmd::Install { name, source, yes })
}

/// Normalize an installation-method hint by name, without probing the filesystem.
fn lexical_normal(p: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Persist a real absolute path, following links before resolving parent components.
fn absolutize(
    source: Source,
    cwd: &Path,
    home: Option<&Path>,
    lang: crate::i18n::Lang,
) -> Result<Source> {
    match source {
        Source::Path { path } => {
            let expanded = list_file::expand_tilde_lang(&path, home, lang)?;
            let resolved = cwd.join(expanded).canonicalize().with_context(|| {
                crate::i18n::tr(lang, crate::i18n::Msg::PluginPathResolve).replace("{path}", &path)
            })?;
            Ok(Source::Path {
                path: resolved.to_string_lossy().into_owned(),
            })
        }
        s => Ok(s),
    }
}

/// Ask whether to install. With `yes` nothing is asked (the details are still printed); without a
/// terminal on stdin and without `yes` there is no one to ask, so it refuses.
pub fn confirm_install_lang(
    name: &str,
    source: &Source,
    lang: Lang,
    yes: bool,
    stdin_is_tty: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<bool> {
    writeln!(
        out,
        "{}",
        tr(lang, Msg::PluginCrateLabel).replace("{name}", name)
    )?;
    writeln!(
        out,
        "{}",
        tr(lang, Msg::PluginSourceLabel).replace("{source}", &source.describe_lang(lang))
    )?;
    writeln!(out, "{}", tr(lang, Msg::PluginPermissions))?;
    if yes {
        return Ok(true);
    }
    if !stdin_is_tty {
        bail!(tr(lang, Msg::PluginConfirmUnavailable));
    }
    write!(out, "{}", tr(lang, Msg::PluginConfirmQuestion))?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// The text of `list`: one block per crate; `lock` maps crate names to their locked versions.
#[cfg(test)]
pub fn list_text_lang(list: &PluginList, lock: &BTreeMap<String, String>, lang: Lang) -> String {
    list_text_with_updates(list, lock, None, lang)
}

fn list_text_with_updates(
    list: &PluginList,
    lock: &BTreeMap<String, String>,
    updates: Option<&update_check::Snapshot>,
    lang: Lang,
) -> String {
    let mut s = String::new();
    if let Some(p) = &list.konoma_path {
        s.push_str(&format!(
            "{}\n",
            tr(lang, Msg::PluginSourceBuild).replace("{path}", p)
        ));
    }
    if list.plugins.is_empty() {
        s.push_str(tr(lang, Msg::PluginListEmpty));
        s.push('\n');
    }
    for (name, src) in &list.plugins {
        let version = lock
            .get(name)
            .map(String::as_str)
            .unwrap_or(tr(lang, Msg::PluginNotBuilt));
        s.push_str(&format!(
            "{name}  {version}\n    {}\n",
            src.describe_lang(lang)
        ));
        if !src.has_update_check() {
            s.push_str(&format!("    {}\n", tr(lang, Msg::PluginNoUpdateCheck)));
        } else if let Some(note) =
            updates.and_then(|u| u.list_note_lang(name, lock.get(name), lang))
        {
            s.push_str(&format!("    {note}\n"));
        }
    }
    if let Some(u) = updates.filter(|u| u.show_checked_time) {
        s.push_str(&format!(
            "{}\n",
            tr(lang, Msg::PluginLastChecked).replace(
                "{time}",
                &konoma_base::epoch::format_epoch_utc(u.checked_at)
            )
        ));
    }
    s
}

fn list_output_with(env: &Env, fetch: update_check::Fetch) -> Result<String> {
    let mut list = PluginList::load_lang(&env.plugins_toml(), env.lang)?;
    let empty = list.plugins.is_empty();
    let mut locked = std::fs::read_to_string(env.lock_path())
        .map(|t| project::locked_versions(&t))
        .unwrap_or_default();
    let record = switch::read_record(&env.data_dir);
    let bundled = !switch::binary_path(&env.data_dir).is_file();
    if let Some(record) = record {
        locked.extend(record.plugins);
    }
    for (name, (compiled, requirement)) in update_check::standard_targets() {
        if bundled {
            locked.insert(name.clone(), compiled.to_string());
        }
        list.plugins.entry(name).or_insert_with(|| Source::Crates {
            version: requirement.to_string(),
        });
    }
    let (config, _) =
        crate::config::Config::load_path_reporting(&env.config_dir.join("config.toml"));
    let mut check = update_check::query_list(&list, &locked, config.external.update_check, fetch);
    if let Some(saved) = update_check::Snapshot::load(&env.data_dir) {
        check.snapshot.checked_at = saved.checked_at;
        check.snapshot.show_checked_time = true;
    }
    let mut text = if bundled {
        format!(
            "{}\n",
            tr(
                env.lang,
                if empty {
                    Msg::PluginListEmpty
                } else {
                    Msg::PluginStandardBundled
                }
            )
        )
    } else {
        String::new()
    };
    text.push_str(&list_text_with_updates(
        &list,
        &locked,
        Some(&check.snapshot),
        env.lang,
    ));
    if let Some(note) = check.status_note(env.lang) {
        text.push_str(&format!("{note}\n"));
    }
    Ok(text)
}

fn cmd_list(env: &Env) -> Result<()> {
    print!("{}", list_output_with(env, update_check::Fetch::http())?);
    Ok(())
}

fn cmd_install(env: &Env, name: String, source: Source, yes: bool, cwd: &Path) -> Result<()> {
    // Read once before asking anything, so that a broken file is reported first.
    let mut candidate = PluginList::load_lang(&env.plugins_toml(), env.lang)?;
    if project::STANDARD_CRATES.contains(&name.as_str()) {
        bail!(tr(env.lang, Msg::PluginAlreadyStandard).replace("{name}", &name));
    }
    let lang = env.lang;
    let source = absolutize(source, cwd, env.home.as_deref(), lang)?;
    if let Source::Path { path } = &source {
        if !list_file::path_form_ok(path) {
            bail!(tr(lang, Msg::PluginPathAbsolute).replace("{path}", path));
        }
    }
    let stdin = std::io::stdin();
    if !yes && !env.stdin_is_tty {
        return confirm_install_lang(
            &name,
            &source,
            lang,
            false,
            false,
            &mut stdin.lock(),
            &mut std::io::stdout(),
        )
        .map(|_| ());
    }
    candidate.plugins.insert(name.clone(), source.clone());
    let authors = build::authors(env, &candidate, &name)?;
    let authors = if authors.is_empty() {
        crate::i18n::tr(lang, crate::i18n::Msg::PluginAuthorsMissing).to_owned()
    } else {
        authors.join(", ")
    };
    println!(
        "{}",
        crate::i18n::tr(lang, crate::i18n::Msg::PluginAuthors).replace("{authors}", &authors)
    );
    if !confirm_install_lang(
        &name,
        &source,
        lang,
        yes,
        env.stdin_is_tty,
        &mut stdin.lock(),
        &mut std::io::stdout(),
    )? {
        println!("{}", tr(lang, Msg::PluginCancelled));
        return Ok(());
    }
    // The lock comes after the question (nobody should wait for someone's answer), and the list
    // is read again under it: another run may have changed it while this one was asking.
    let _lock = write_lock(env)?;
    let mut list = PluginList::load_lang(&env.plugins_toml(), env.lang)?;
    list.plugins.insert(name, source);
    build::rebuild(env, &list, Update::Nothing, true)
}

/// The lock that every command that changes anything holds (see `lock`).
fn write_lock(env: &Env) -> Result<lock::PluginLock> {
    lock::acquire_lang(&env.data_dir, env.lang, &mut std::io::stderr())
}

fn cmd_update(env: &Env, which: Option<String>) -> Result<()> {
    let _lock = write_lock(env)?;
    let list = PluginList::load_lang(&env.plugins_toml(), env.lang)?;
    let update = match which {
        None => Update::All,
        Some(n)
            if list.plugins.contains_key(&n) || project::STANDARD_CRATES.contains(&n.as_str()) =>
        {
            Update::One(n)
        }
        Some(n) => bail!(tr(env.lang, Msg::PluginNotInstalled).replace("{name}", &n)),
    };
    build::rebuild(env, &list, update, false)
}

fn cmd_remove(env: &Env, name: String) -> Result<()> {
    let _lock = write_lock(env)?;
    let mut list = PluginList::load_lang(&env.plugins_toml(), env.lang)?;
    if list.plugins.remove(&name).is_none() {
        bail!(tr(env.lang, Msg::PluginNotInstalled).replace("{name}", &name));
    }
    if list.plugins.is_empty() {
        // Nothing to link in: no rebuild, and the plain konoma starts again.
        // The program first: the standard konoma then runs even if the list cannot be written.
        // Running the same `remove` again finishes the job, since the list still names the crate.
        //
        // This path does not go through `rebuild`, so the signals are taken here, as `commit`
        // has them: noticed and put off, not obeyed. A Ctrl-C between the two steps would
        // otherwise end the process after the program is gone and before the list is saved,
        // without the explanation that only the error below gives.
        build::catch_interrupts(&env.interrupted);
        build::remove_program_lang(&env.data_dir, env.lang)?;
        list.save_lang(&env.plugins_toml(), env.lang)
            .with_context(|| tr(env.lang, Msg::PluginRemoveListFailed).replace("{name}", &name))?;
        println!(
            "{}",
            tr(env.lang, Msg::PluginRemovedLast).replace("{name}", &name)
        );
        return Ok(());
    }
    build::rebuild(env, &list, Update::Nothing, true)
}

fn cmd_clean(env: &Env) -> Result<()> {
    // Always serialize, even on the first run: a directory-existence check before acquiring the
    // lock could race with metadata creating its first work directory.
    let _lock = write_lock(env)?;
    let removed = build::clean_lang(&env.data_dir, env.lang)?;
    if removed.is_empty() {
        println!("{}", tr(env.lang, Msg::PluginCleanEmpty));
    }
    for p in removed {
        println!(
            "{}",
            tr(env.lang, Msg::PluginRemovedPath).replace("{path}", &p.display().to_string())
        );
    }
    Ok(())
}

fn cli_language(
    config_dir: &std::path::Path,
    os: impl FnOnce() -> crate::i18n::Lang,
) -> crate::i18n::Lang {
    let (config, _) = crate::config::Config::load_path_reporting(&config_dir.join("config.toml"));
    crate::i18n::config_language_with(&config, os)
}

fn production_env(interrupted: Arc<AtomicBool>) -> Result<Env> {
    use std::io::IsTerminal;
    production_env_from_dirs(
        konoma_base::xdg::xdg_config_home().map(|d| d.join("konoma")),
        konoma_base::xdg::data_dir(),
        interrupted,
        crate::i18n::resolve_lang("auto"),
        std::io::stdin().is_terminal(),
    )
}

fn production_env_from_dirs(
    config: Option<std::path::PathBuf>,
    data: Option<std::path::PathBuf>,
    interrupted: Arc<AtomicBool>,
    fallback: Lang,
    stdin_is_tty: bool,
) -> Result<Env> {
    // Until the config can be located, only the OS/default language is available.
    let config_dir = config.context(tr(fallback, Msg::PluginConfigDirMissing))?;
    let lang = cli_language(&config_dir, || fallback);
    let data_dir = data.context(tr(lang, Msg::PluginDataDirMissing))?;
    Ok(Env {
        lang,
        config_dir,
        data_dir,
        home: std::env::var_os("HOME").map(Into::into),
        cargo_env: std::env::var_os("CARGO"),
        path_env: std::env::var_os("PATH"),
        konoma_version: env!("CARGO_PKG_VERSION").to_string(),
        stdin_is_tty,
        interrupted,
    })
}

/// Run `konoma plugin ...` (`args` is the whole argument list, `args[0]` the program). Returns the
/// process exit code: 0, 1 for a failure, 2 for a command line that makes no sense, 130 when
/// interrupted.
pub fn run(args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
    // Help stays English and configuration-free, just like the application's main help.
    if matches!(
        args.get(2).map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        print!("{}", usage(false));
        return 0;
    }
    // Usage errors do not require HOME or writable config/data roots.
    let lang = konoma_base::xdg::xdg_config_home()
        .map(|dir| cli_language(&dir.join("konoma"), || crate::i18n::resolve_lang("auto")))
        .unwrap_or_else(|| crate::i18n::resolve_lang("auto"));
    let cmd = match parse_lang(args.get(2..).unwrap_or(&[]), lang) {
        Ok(Cmd::Help) => {
            print!("{}", usage(false));
            return 0;
        }
        Ok(c) => c,
        Err(complaint) => {
            eprintln!("konoma plugin: {complaint}\n");
            eprint!("{}", usage(cwd.join("plugin").exists()));
            return 2;
        }
    };
    let env = match production_env(Arc::new(AtomicBool::new(false))) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("konoma plugin: {error:#}");
            return 1;
        }
    };
    let result = match cmd {
        Cmd::Install { name, source, yes } => cmd_install(&env, name, source, yes, &cwd),
        Cmd::Update(which) => cmd_update(&env, which),
        Cmd::Remove(name) => cmd_remove(&env, name),
        Cmd::List => cmd_list(&env),
        Cmd::Clean => cmd_clean(&env),
        Cmd::Help => Ok(()),
    };
    match result {
        Ok(()) => 0,
        Err(e) if e.downcast_ref::<Interrupted>().is_some() => {
            eprintln!("konoma plugin: {e}");
            130
        }
        Err(e) => {
            eprintln!("konoma plugin: {e:#}");
            1
        }
    }
}

#[cfg(test)]
pub fn parse(args: &[String]) -> Result<Cmd, String> {
    parse_lang(args, Lang::En)
}
#[cfg(test)]
pub fn confirm_install(
    name: &str,
    source: &Source,
    yes: bool,
    tty: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<bool> {
    confirm_install_lang(name, source, Lang::En, yes, tty, input, out)
}
#[cfg(test)]
pub fn list_text(list: &PluginList, lock: &BTreeMap<String, String>) -> String {
    list_text_lang(list, lock, Lang::En)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    mod pipe_fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/process_fixture.rs"
        ));
    }

    #[test]
    fn post_exit_pipe_error_matches_sdk_message_contract() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exit 0"])
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let pending = pipe_fixture::PendingHelper::attach(&mut command, true, false);
        let mut child = konoma_plugin::proc::process::ChildGuard::new(command.spawn().unwrap());
        let mut helper = pending.ready();
        let result = konoma_plugin::proc::process::collect_output(
            &mut child,
            || false,
            None,
            usize::MAX,
            Duration::ZERO,
        );
        helper.stop();
        assert!(helper.gone());
        let error = result
            .err()
            .expect("escaped holder must reach the post-exit drain limit");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        // The English diagnostic was public CLI output before fix-9; pin it independently.
        let diagnostic = error.to_string();
        assert_eq!(
            process_error(error, Lang::Jp).to_string(),
            "子の終了後も出力パイプが開いたままです"
        );
        assert_eq!(diagnostic, "output pipes remained open after child exit");
    }

    #[test]
    fn list_output_language_walk() {
        let dir = crate::test_support::unique_tmp("list-output-language");
        let config = dir.join("config");
        let data = dir.join("data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(config.join("plugins.toml"), "[plugins.konoma-foo]\nversion = '^0.3'\n[plugins.local]\npath = '/fixture'\n[plugins.git-plugin]\ngit = 'https://example.invalid/plugin'\n").unwrap();
        std::fs::write(
            data.join("Cargo.lock"),
            "[[package]]\nname = 'konoma-foo'\nversion = '0.3.2'\n",
        )
        .unwrap();
        std::fs::write(data.join("plugin-updates.toml"), "checked_at = 100\n").unwrap();
        for lang in [Lang::En, Lang::Jp] {
            let env = Env {
                lang,
                config_dir: config.clone(),
                data_dir: data.clone(),
                home: None,
                cargo_env: None,
                path_env: None,
                konoma_version: "0.31.2".into(),
                stdin_is_tty: false,
                interrupted: Arc::new(AtomicBool::new(false)),
            };
            for (enabled, fails) in [(true, false), (true, true), (false, false)] {
                std::fs::write(
                    config.join("config.toml"),
                    format!("[external]\nupdate_check = {enabled}\n"),
                )
                .unwrap();
                let text = list_output_with(&env, update_check::Fetch::injected(move |url| {
                    assert!(enabled, "disabled fetch");
                    if fails { return None }
                    let rows = if url.ends_with("/konoma-foo") { vec!["0.3.3", "1.0.0"] } else { vec!["0.1.0"] };
                    Some(rows.iter().map(|version| serde_json::json!({"vers": version, "yanked": false, "deps": []}).to_string()).collect::<Vec<_>>().join("\n"))
                })).unwrap();
                assert!(text.contains("konoma-office  0.1.0"), "{text}");
                if lang == Lang::Jp {
                    crate::test_support::plugin_language::assert_japanese_plugin_prose(
                        "list status",
                        &text,
                    );
                } else {
                    crate::test_support::plugin_english_lower::assert_english_context(
                        "list bundled header",
                        &text,
                        "Standard plugins bundled with konoma are in use.",
                    );
                    assert!(text.contains("    no update check\n"), "{text}");
                }
                let expected = match (lang, enabled, fails) {
                    (Lang::En, false, _) => "Plugin update checking is turned off.",
                    (Lang::Jp, false, _) => "プラグインの更新確認は無効です。",
                    (Lang::En, true, true) => "Could not check for plugin updates.",
                    (Lang::Jp, true, true) => "プラグインの更新を確認できませんでした。",
                    (Lang::En, true, false) => {
                        "update available: 0.3.3 / newer 1.0.0 needs a newer konoma"
                    }
                    (Lang::Jp, true, false) => {
                        "更新あり: 0.3.3 / 新しい版 1.0.0 には konoma の更新が必要です"
                    }
                };
                assert!(text.contains(expected), "{text}");
                if enabled && !fails {
                    assert!(
                        text.contains(if lang == Lang::En {
                            "    up to date\n"
                        } else {
                            "    最新です\n"
                        }),
                        "{text}"
                    );
                }
                assert_eq!(
                    std::fs::read_to_string(data.join("plugin-updates.toml")).unwrap(),
                    "checked_at = 100\n"
                );
                assert!(!data.join("plugin-updates.lock").exists());
            }
        }
    }

    #[test]
    fn confirmation_and_list_language_walk() {
        for lang in [Lang::Jp, Lang::En] {
            for source in [
                Source::Crates {
                    version: "0.3".into(),
                },
                Source::Git {
                    url: "https://example.invalid/plugin".into(),
                    branch: Some("dev".into()),
                    rev: None,
                },
                Source::Git {
                    url: "https://example.invalid/plugin".into(),
                    branch: None,
                    rev: Some("abc123".into()),
                },
                Source::Path {
                    path: "/fixture".into(),
                },
            ] {
                let english_source = match &source {
                    Source::Crates { version } => format!("crates.io, version {version}"),
                    Source::Git { url, branch, rev } => {
                        let mut text = format!("git {url}");
                        if let Some(branch) = branch {
                            text.push_str(&format!(", branch {branch}"));
                        }
                        if let Some(rev) = rev {
                            text.push_str(&format!(", rev {rev}"));
                        }
                        text
                    }
                    Source::Path { path } => format!("path {path}"),
                };
                let no_check = !source.has_update_check();
                let mut output = Vec::new();
                assert!(!confirm_install_lang(
                    "konoma-foo",
                    &source,
                    lang,
                    false,
                    true,
                    &mut Cursor::new(b"n\n"),
                    &mut output
                )
                .unwrap());
                let output = String::from_utf8(output).unwrap();
                assert!(output.contains("[y/N]"));
                let mut list = PluginList {
                    konoma_path: Some("/fixture".into()),
                    ..Default::default()
                };
                list.plugins.insert("konoma-foo".into(), source);
                let list = list_text_lang(&list, &BTreeMap::new(), lang);
                if lang == Lang::Jp {
                    for (name, text) in [("confirmation", output), ("list", list)] {
                        crate::test_support::plugin_language::assert_japanese_plugin_prose(
                            name, &text,
                        );
                    }
                } else {
                    assert_eq!(output, format!("Plugin crate:  konoma-foo\nSource:        {english_source}\nA plugin runs with your permissions, and its build scripts run on this machine while konoma is rebuilt.\nInstall it? [y/N] "));
                    let mut expected_list = format!("(konoma is built from the source at /fixture)\nkonoma-foo  not built yet\n    {english_source}\n");
                    if no_check {
                        expected_list.push_str("    no update check\n");
                    }
                    assert_eq!(list, expected_list);
                    for text in [&output, &list] {
                        assert!(
                            !text.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c)),
                            "Japanese in English formatter output: {text}"
                        );
                    }
                }
            }
            let snapshot = update_check::Snapshot::language_fixture();
            let list = PluginList::parse(
                "[plugins.konoma-foo]\nversion = '0.3'\n[plugins.konoma-bar]\nversion = '1'",
            )
            .unwrap();
            let text = list_text_with_updates(
                &list,
                &BTreeMap::from([
                    ("konoma-foo".into(), "0.3.2".into()),
                    ("konoma-bar".into(), "1.0.0".into()),
                ]),
                Some(&snapshot),
                lang,
            );
            if lang == Lang::Jp {
                crate::test_support::plugin_language::assert_japanese_plugin_prose(
                    "update list",
                    &text,
                );
            } else {
                // Literal prose from pre-fix-9 mod.rs::list_text_with_updates and
                // update_check.rs::Snapshot::list_note, not from the current catalog.
                assert_eq!(text, "konoma-bar  1.0.0\n    crates.io, version 1\n    newer 2.0.0 needs a newer konoma\nkonoma-foo  0.3.2\n    crates.io, version 0.3\n    update available: 0.3.3\nLast checked: 1970-01-01 00:00:00 UTC\n");
                assert!(!text.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c)));
            }
        }
    }

    #[test]
    fn absent_config_uses_the_default_auto_language() {
        let dir = konoma_test_support::unique_tmp("cli-default-language");
        // The locale source is injected, so the machine's locale cannot affect this test.
        for (locale, expected) in [
            ("ja-JP", crate::i18n::Lang::Jp),
            ("en-US", crate::i18n::Lang::En),
        ] {
            assert_eq!(
                super::cli_language(&dir, || crate::i18n::Lang::from_lang_tag(locale)),
                expected
            );
        }
    }

    #[test]
    fn bootstrap_error_language_walk() {
        for lang in [Lang::Jp, Lang::En] {
            let error =
                production_env_from_dirs(None, None, Arc::new(AtomicBool::new(false)), lang, false)
                    .err()
                    .unwrap();
            let dir = konoma_test_support::unique_tmp("bootstrap-language");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("config.toml"),
                format!(
                    "[ui]\nlang = '{}'\n",
                    if lang == Lang::Jp { "jp" } else { "en" }
                ),
            )
            .unwrap();
            let configured = production_env_from_dirs(
                Some(dir.to_path_buf()),
                None,
                Arc::new(AtomicBool::new(false)),
                Lang::En,
                false,
            )
            .err()
            .unwrap();
            if lang == Lang::Jp {
                for error in [error, configured] {
                    crate::test_support::plugin_language::assert_japanese_plugin_prose(
                        "bootstrap",
                        &error.to_string(),
                    );
                }
            } else {
                assert_eq!(
                    error.to_string(),
                    "cannot find the config directory (HOME is not set)"
                );
                assert_eq!(
                    configured.to_string(),
                    "cannot find the data directory (HOME is not set)"
                );
                for error in [error, configured] {
                    crate::test_support::plugin_english_lower::assert_english_context(
                        "bootstrap",
                        &error.to_string(),
                        &error.to_string(),
                    );
                }
            }
        }
    }

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plugin_as_the_first_argument_is_always_the_command() {
        assert!(is_plugin_command(&a(&["konoma", "plugin"])));
        assert!(is_plugin_command(&a(&["konoma", "plugin", "list"])));
        assert!(!is_plugin_command(&a(&["konoma", "./plugin"])));
        assert!(!is_plugin_command(&a(&["konoma", "plugins"])));
        assert!(!is_plugin_command(&a(&["konoma", "x", "plugin"])));
        assert!(!is_plugin_command(&a(&["konoma"])));
        assert!(!is_plugin_command(&a(&["konoma", "--plugins"])));
    }

    #[test]
    fn no_command_and_unknown_commands_are_complaints() {
        assert!(parse(&[]).is_err());
        let e = parse(&a(&["frobnicate"])).unwrap_err();
        assert!(e.contains("frobnicate"), "{e}");
        assert!(parse(&a(&["LIST"])).is_err(), "commands are lower case");
    }

    #[test]
    fn simple_commands_and_their_extra_arguments() {
        assert_eq!(parse(&a(&["list"])), Ok(Cmd::List));
        assert_eq!(parse(&a(&["clean"])), Ok(Cmd::Clean));
        assert_eq!(parse(&a(&["help"])), Ok(Cmd::Help));
        assert_eq!(parse(&a(&["--help"])), Ok(Cmd::Help));
        assert!(parse(&a(&["list", "x"])).is_err());
        assert!(parse(&a(&["clean", "x"])).is_err());
        assert_eq!(parse(&a(&["update"])), Ok(Cmd::Update(None)));
        assert_eq!(
            parse(&a(&["update", "konoma-foo"])),
            Ok(Cmd::Update(Some("konoma-foo".into())))
        );
        assert!(parse(&a(&["update", "a", "b"])).is_err());
        assert!(parse(&a(&["update", "a b"])).is_err());
        assert_eq!(
            parse(&a(&["remove", "konoma-foo"])),
            Ok(Cmd::Remove("konoma-foo".into()))
        );
        assert!(parse(&a(&["remove"])).is_err());
        assert!(parse(&a(&["remove", "a", "b"])).is_err());
        assert!(parse(&a(&["remove", "../x"])).is_err());
    }

    #[test]
    fn install_forms() {
        assert_eq!(
            parse(&a(&["install", "konoma-foo"])),
            Ok(Cmd::Install {
                name: "konoma-foo".into(),
                source: Source::Crates {
                    version: "*".into()
                },
                yes: false
            })
        );
        assert_eq!(
            parse(&a(&["install", "konoma-foo@0.3", "--yes"])),
            Ok(Cmd::Install {
                name: "konoma-foo".into(),
                source: Source::Crates {
                    version: "0.3".into()
                },
                yes: true
            })
        );
        assert_eq!(
            parse(&a(&[
                "install",
                "-y",
                "konoma-foo",
                "--git",
                "https://x/y",
                "--branch",
                "dev"
            ])),
            Ok(Cmd::Install {
                name: "konoma-foo".into(),
                source: Source::Git {
                    url: "https://x/y".into(),
                    branch: Some("dev".into()),
                    rev: None
                },
                yes: true
            })
        );
        assert_eq!(
            parse(&a(&["install", "konoma-foo", "--path", "rel/dir"])),
            Ok(Cmd::Install {
                name: "konoma-foo".into(),
                source: Source::Path {
                    path: "rel/dir".into()
                },
                yes: false
            })
        );
    }

    #[test]
    fn install_mistakes_are_complaints() {
        for bad in [
            vec!["install"],
            vec!["install", "a", "b"],
            vec!["install", "a@"],
            vec!["install", "bad name"],
            vec!["install", "a", "--git"],
            vec!["install", "a", "--git", "--yes"],
            vec!["install", "a", "--git", "u", "--path", "/p"],
            vec!["install", "a@1", "--git", "u"],
            vec!["install", "a@1", "--path", "/p"],
            vec!["install", "a", "--branch", "b"],
            vec!["install", "a", "--rev", "r"],
            vec!["install", "a", "--git", "u", "--branch", "b", "--rev", "r"],
            vec!["install", "a", "--bogus"],
        ] {
            assert!(parse(&a(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn relative_absolute_and_tilde_paths_resolve() {
        let d = konoma_test_support::unique_tmp("plugin-path-forms");
        std::fs::create_dir_all(d.join("p/q")).unwrap();
        let expected = d.join("p/q").canonicalize().unwrap();
        for path in [
            "./p/./q/".to_string(),
            "~/p/q".to_string(),
            expected.to_string_lossy().into_owned(),
        ] {
            assert_eq!(
                absolutize(Source::Path { path }, &d, Some(&d), crate::i18n::Lang::En).unwrap(),
                Source::Path {
                    path: expected.to_string_lossy().into_owned()
                }
            );
        }
    }

    #[test]
    fn usage_lists_every_command_and_hints_at_a_plugin_folder() {
        let plain = usage(false);
        for c in ["install", "update", "remove", "list", "clean"] {
            assert!(plain.contains(c), "{c}");
        }
        assert!(!plain.contains("konoma ./plugin"));
        assert!(usage(true).contains("konoma ./plugin"));
    }

    fn confirm(yes: bool, tty: bool, input: &str) -> (Result<bool>, String) {
        let mut out = Vec::new();
        let r = confirm_install(
            "konoma-foo",
            &Source::Crates {
                version: "0.3".into(),
            },
            yes,
            tty,
            &mut Cursor::new(input.as_bytes().to_vec()),
            &mut out,
        );
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn the_confirmation_shows_name_range_and_origin() {
        let (_, out) = confirm(true, false, "");
        assert!(
            out.contains("konoma-foo") && out.contains("0.3") && out.contains("crates.io"),
            "{out}"
        );
        let (_, out) = confirm_text_for_git();
        assert!(
            out.contains("https://x/y") && out.contains("branch dev"),
            "{out}"
        );
    }

    fn confirm_text_for_git() -> (Result<bool>, String) {
        let mut out = Vec::new();
        let r = confirm_install(
            "konoma-bar",
            &Source::Git {
                url: "https://x/y".into(),
                branch: Some("dev".into()),
                rev: None,
            },
            true,
            false,
            &mut Cursor::new(Vec::new()),
            &mut out,
        );
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn yes_skips_the_question_even_without_a_terminal() {
        let (r, out) = confirm(true, false, "");
        assert!(r.unwrap());
        assert!(!out.contains("[y/N]"));
    }

    #[test]
    fn without_a_terminal_and_without_yes_it_refuses() {
        let (r, out) = confirm(false, false, "y\n");
        let e = r.unwrap_err().to_string();
        assert!(e.contains("--yes"), "{e}");
        assert!(!out.contains("[y/N]"), "it did not ask");
    }

    #[test]
    fn on_a_terminal_only_y_and_yes_say_yes() {
        for (input, want) in [
            ("y\n", true),
            ("Y\n", true),
            ("yes\n", true),
            (" YES \n", true),
            ("n\n", false),
            ("\n", false),
            ("", false),
            ("yep\n", false),
            ("no\n", false),
        ] {
            let (r, out) = confirm(false, true, input);
            assert_eq!(r.unwrap(), want, "{input:?}");
            assert!(out.contains("[y/N]"));
        }
    }

    #[test]
    fn list_shows_versions_origins_and_the_missing_update_check() {
        let list = PluginList::parse(
            "[plugins.konoma-a]\nversion = \"0.3\"\n[plugins.konoma-b]\ngit = \"https://x\"\n[plugins.konoma-c]\npath = \"/c\"\n",
        )
        .unwrap();
        let mut lock = BTreeMap::new();
        lock.insert("konoma-a".to_string(), "0.3.2".to_string());
        lock.insert("konoma-b".to_string(), "0.1.0".to_string());
        let t = list_text(&list, &lock);
        assert!(t.contains("konoma-a  0.3.2"), "{t}");
        assert!(t.contains("konoma-b  0.1.0"), "{t}");
        assert!(t.contains("konoma-c  not built yet"), "{t}");
        // The note belongs to git and path entries only.
        assert_eq!(t.matches("no update check").count(), 2, "{t}");
        let a_block = t.split("konoma-b").next().unwrap();
        assert!(!a_block.contains("no update check"), "{t}");
    }

    /// Removing the last plugin takes the signals before it deletes anything: afterwards a SIGTERM
    /// to this very process is only noted (without the handler it would end the test process).
    /// The path has no other signal-dependent step to observe from outside without racing the
    /// clock, so what is checked is that the handler is in place once the command has run.
    #[test]
    fn removing_the_last_plugin_puts_off_signals_instead_of_dying() {
        let d = konoma_test_support::unique_tmp("rm-last-signals");
        std::fs::create_dir_all(&d).unwrap();
        let env = Env {
            config_dir: d.join("config"),
            data_dir: d.join("data"),
            home: None,
            cargo_env: None,
            path_env: None,
            konoma_version: "0.0.0".into(),
            lang: crate::i18n::Lang::En,
            stdin_is_tty: false,
            interrupted: Arc::new(AtomicBool::new(false)),
        };
        std::fs::create_dir_all(&env.config_dir).unwrap();
        std::fs::write(
            env.plugins_toml(),
            "[plugins.konoma-foo]\nversion = \"1\"\n",
        )
        .unwrap();
        cmd_remove(&env, "konoma-foo".into()).unwrap();
        assert!(!env.interrupted.load(std::sync::atomic::Ordering::SeqCst));
        // SAFETY: raise(3) on this process with a handler installed by the call above.
        unsafe {
            libc::raise(libc::SIGHUP);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !env.interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline, "signal not noted");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn list_of_nothing_says_the_standard_build() {
        let t = list_text(&PluginList::default(), &BTreeMap::new());
        assert!(t.contains("standard build"), "{t}");
        let with_path = PluginList::parse("[konoma]\npath = \"/k\"\n").unwrap();
        assert!(list_text(&with_path, &BTreeMap::new()).contains("/k"));
    }
}
