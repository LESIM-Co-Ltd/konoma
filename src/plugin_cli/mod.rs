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

use anyhow::{bail, Context, Result};

use build::{Env, Interrupted, Update};
use list_file::{PluginList, Source};

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
    let mut s = String::from(
        "Usage: konoma plugin <command>\n\
         \n\
         Commands:\n\
         \x20 install <crate>[@version] [--git URL [--branch B | --rev R]] [--path DIR] [-y|--yes]\n\
         \x20                          Add a plugin crate and rebuild konoma with it\n\
         \x20 update [<crate>]         Take newer versions (within semver) and rebuild\n\
         \x20 remove <crate>           Take a plugin out and rebuild (the last one: back to plain konoma)\n\
         \x20 list                     Show the plugin crates, where they come from, and their versions\n\
         \x20 clean                    Delete the kept build cache ([plugins] keep_build_cache = true)\n\
         \n\
         The list is plugins.toml next to config.toml. The rebuilt konoma is kept in konoma's data\n\
         directory; the `konoma` command starts it automatically.\n",
    );
    if plugin_entry_here {
        s.push_str(
            "\nThere is a folder or file named `plugin` here; to open that folder, run `konoma ./plugin`.\n",
        );
    }
    s
}

/// Parse the arguments after `plugin`. `Err` is the complaint to print above the usage.
pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let Some(sub) = args.first() else {
        return Err("a command is needed".into());
    };
    let rest = &args[1..];
    match sub.as_str() {
        "help" | "--help" | "-h" => Ok(Cmd::Help),
        "list" => no_args(rest, "list").map(|_| Cmd::List),
        "clean" => no_args(rest, "clean").map(|_| Cmd::Clean),
        "remove" => match rest {
            [n] if list_file::valid_crate_name(n) => Ok(Cmd::Remove(n.clone())),
            [n] => Err(format!("`{n}` is not a crate name")),
            [] => Err("remove needs a crate name".into()),
            _ => Err("remove takes one crate name".into()),
        },
        "update" => match rest {
            [] => Ok(Cmd::Update(None)),
            [n] if list_file::valid_crate_name(n) => Ok(Cmd::Update(Some(n.clone()))),
            [n] => Err(format!("`{n}` is not a crate name")),
            _ => Err("update takes at most one crate name".into()),
        },
        "install" => parse_install(rest),
        other => Err(format!("unknown command `{other}`")),
    }
}

fn no_args(rest: &[String], cmd: &str) -> Result<(), String> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(format!("{cmd} takes no arguments"))
    }
}

fn parse_install(rest: &[String]) -> Result<Cmd, String> {
    let (mut spec, mut git, mut branch, mut rev, mut path) = (None, None, None, None, None);
    let mut yes = false;
    let mut i = 0;
    let value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        rest.get(*i)
            .filter(|v| !v.starts_with("--") && !v.is_empty())
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while i < rest.len() {
        match rest[i].as_str() {
            "-y" | "--yes" => yes = true,
            "--git" => git = Some(value(&mut i, "--git")?),
            "--branch" => branch = Some(value(&mut i, "--branch")?),
            "--rev" => rev = Some(value(&mut i, "--rev")?),
            "--path" => path = Some(value(&mut i, "--path")?),
            a if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            a => {
                if spec.replace(a.to_string()).is_some() {
                    return Err("install takes one crate".into());
                }
            }
        }
        i += 1;
    }
    let spec = spec.ok_or("install needs a crate name")?;
    let (name, version) = match spec.split_once('@') {
        Some((n, v)) if !v.is_empty() => (n.to_string(), Some(v.to_string())),
        Some(_) => return Err("a version is needed after @".into()),
        None => (spec, None),
    };
    if !list_file::valid_crate_name(&name) {
        return Err(format!("`{name}` is not a crate name"));
    }
    if git.is_some() && path.is_some() {
        return Err("--git and --path are alternatives".into());
    }
    if (git.is_some() || path.is_some()) && version.is_some() {
        return Err("@version goes with crates.io; with --git or --path leave it out".into());
    }
    if git.is_none() && (branch.is_some() || rev.is_some()) {
        return Err("--branch and --rev go with --git".into());
    }
    if branch.is_some() && rev.is_some() {
        return Err("use --branch or --rev, not both".into());
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
            let expanded = list_file::expand_tilde(&path, home)?;
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
pub fn confirm_install(
    name: &str,
    source: &Source,
    yes: bool,
    stdin_is_tty: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<bool> {
    writeln!(out, "Plugin crate:  {name}")?;
    writeln!(out, "Source:        {}", source.describe())?;
    writeln!(
        out,
        "A plugin runs with your permissions, and its build scripts run on this machine while konoma is rebuilt."
    )?;
    if yes {
        return Ok(true);
    }
    if !stdin_is_tty {
        bail!(
            "cannot ask for confirmation: stdin is not a terminal (pass --yes to install anyway)"
        );
    }
    write!(out, "Install it? [y/N] ")?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// The text of `list`: one block per crate; `lock` maps crate names to their locked versions.
pub fn list_text(list: &PluginList, lock: &BTreeMap<String, String>) -> String {
    list_text_with_updates(list, lock, None)
}

fn list_text_with_updates(
    list: &PluginList,
    lock: &BTreeMap<String, String>,
    updates: Option<&update_check::Snapshot>,
) -> String {
    let mut s = String::new();
    if let Some(p) = &list.konoma_path {
        s.push_str(&format!("(konoma is built from the source at {p})\n"));
    }
    if list.plugins.is_empty() {
        s.push_str("No plugin crates are installed: konoma is the standard build.\n");
    }
    for (name, src) in &list.plugins {
        let version = lock
            .get(name)
            .map(String::as_str)
            .unwrap_or("not built yet");
        s.push_str(&format!("{name}  {version}\n    {}\n", src.describe()));
        if !src.has_update_check() {
            s.push_str("    no update check\n");
        } else if let Some(note) = updates.and_then(|u| u.list_note(name, lock.get(name))) {
            s.push_str(&format!("    {note}\n"));
        }
    }
    if let Some(u) = updates {
        s.push_str(&format!(
            "Last checked: {}\n",
            konoma_base::epoch::format_epoch_utc(u.checked_at)
        ));
    }
    s
}

fn cmd_list(env: &Env) -> Result<()> {
    let list = PluginList::load(&env.plugins_toml())?;
    let lock = std::fs::read_to_string(env.lock_path())
        .map(|t| project::locked_versions(&t))
        .unwrap_or_default();
    let text = match update_check::Snapshot::load(&env.data_dir) {
        Some(updates) => list_text_with_updates(&list, &lock, Some(&updates)),
        None => list_text(&list, &lock),
    };
    print!("{text}");
    Ok(())
}

fn cmd_install(env: &Env, name: String, source: Source, yes: bool, cwd: &Path) -> Result<()> {
    // Read once before asking anything, so that a broken file is reported first.
    let mut candidate = PluginList::load(&env.plugins_toml())?;
    if project::STANDARD_CRATES.contains(&name.as_str()) {
        bail!("`{name}` is part of the standard build already; nothing to install");
    }
    let lang = env.lang;
    let source = absolutize(source, cwd, env.home.as_deref(), lang)?;
    if let Source::Path { path } = &source {
        if !list_file::path_form_ok(path) {
            bail!("`{path}` is not an absolute path");
        }
    }
    let stdin = std::io::stdin();
    if !yes && !env.stdin_is_tty {
        return confirm_install(
            &name,
            &source,
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
    if !confirm_install(
        &name,
        &source,
        yes,
        env.stdin_is_tty,
        &mut stdin.lock(),
        &mut std::io::stdout(),
    )? {
        println!("Cancelled.");
        return Ok(());
    }
    // The lock comes after the question (nobody should wait for someone's answer), and the list
    // is read again under it: another run may have changed it while this one was asking.
    let _lock = write_lock(env)?;
    let mut list = PluginList::load(&env.plugins_toml())?;
    list.plugins.insert(name, source);
    build::rebuild(env, &list, Update::Nothing, true)
}

/// The lock that every command that changes anything holds (see `lock`).
fn write_lock(env: &Env) -> Result<lock::PluginLock> {
    lock::acquire(&env.data_dir, &mut std::io::stderr())
}

fn cmd_update(env: &Env, which: Option<String>) -> Result<()> {
    let _lock = write_lock(env)?;
    let list = PluginList::load(&env.plugins_toml())?;
    let update = match which {
        None => Update::All,
        Some(n)
            if list.plugins.contains_key(&n) || project::STANDARD_CRATES.contains(&n.as_str()) =>
        {
            Update::One(n)
        }
        Some(n) => bail!("`{n}` is not installed (see `konoma plugin list`)"),
    };
    build::rebuild(env, &list, update, false)
}

fn cmd_remove(env: &Env, name: String) -> Result<()> {
    let _lock = write_lock(env)?;
    let mut list = PluginList::load(&env.plugins_toml())?;
    if list.plugins.remove(&name).is_none() {
        bail!("`{name}` is not installed (see `konoma plugin list`)");
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
        build::remove_program(&env.data_dir)?;
        list.save(&env.plugins_toml()).with_context(|| {
            format!(
                "the rebuilt konoma is removed (konoma is the standard build again), but plugins.toml could not be updated and still lists `{name}`.\n\
                 Run `konoma plugin remove {name}` again once that is fixed, or delete its entry from the file"
            )
        })?;
        println!(
            "Removed `{name}`. No plugin crates are left; konoma is the standard build again."
        );
        return Ok(());
    }
    build::rebuild(env, &list, Update::Nothing, true)
}

fn cmd_clean(env: &Env) -> Result<()> {
    // Always serialize, even on the first run: a directory-existence check before acquiring the
    // lock could race with metadata creating its first work directory.
    let _lock = write_lock(env)?;
    let removed = build::clean(&env.data_dir)?;
    if removed.is_empty() {
        println!("No build cache to remove.");
    }
    for p in removed {
        println!("Removed {}", p.display());
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
    let config_dir = konoma_base::xdg::xdg_config_home()
        .map(|d| d.join("konoma"))
        .context("cannot find the config directory (HOME is not set)")?;
    let data_dir =
        konoma_base::xdg::data_dir().context("cannot find the data directory (HOME is not set)")?;
    let lang = cli_language(&config_dir, || crate::i18n::resolve_lang("auto"));
    Ok(Env {
        lang,
        config_dir,
        data_dir,
        home: std::env::var_os("HOME").map(Into::into),
        cargo_env: std::env::var_os("CARGO"),
        path_env: std::env::var_os("PATH"),
        konoma_version: env!("CARGO_PKG_VERSION").to_string(),
        stdin_is_tty: std::io::stdin().is_terminal(),
        interrupted,
    })
}

/// Run `konoma plugin ...` (`args` is the whole argument list, `args[0]` the program). Returns the
/// process exit code: 0, 1 for a failure, 2 for a command line that makes no sense, 130 when
/// interrupted.
pub fn run(args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
    let cmd = match parse(args.get(2..).unwrap_or(&[])) {
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
    let interrupted = Arc::new(AtomicBool::new(false));
    let result = production_env(interrupted).and_then(|env| match cmd {
        Cmd::Install { name, source, yes } => cmd_install(&env, name, source, yes, &cwd),
        Cmd::Update(which) => cmd_update(&env, which),
        Cmd::Remove(name) => cmd_remove(&env, name),
        Cmd::List => cmd_list(&env),
        Cmd::Clean => cmd_clean(&env),
        Cmd::Help => Ok(()),
    });
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
mod tests {
    use super::*;
    use std::io::Cursor;

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
