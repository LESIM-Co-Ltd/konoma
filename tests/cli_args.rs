//! The `konoma` command's arguments, through the real binary: what the library split
//! (`konoma::Host` + a short `main`) must not change. None of these touch a terminal.

#[path = "support/command.rs"]
mod command;
use command::CommandOutput;
use std::process::{Command, Output};

mod environment {
    include!("support/environment.rs");
}

fn command(args: &[&str], env: &environment::Environment) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_konoma"));
    env.configure(&mut command);
    command.args(args).stdin(std::process::Stdio::null());
    command
}

#[test]
fn commands_isolate_all_user_and_temporary_roots() {
    let env = environment::Environment::new("cli-roots");
    let cmd = command(&["--plugins"], &env);
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
    ] {
        let value = cmd
            .get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .unwrap_or_else(|| panic!("{key} is inherited or unset"));
        assert!(
            std::path::Path::new(value).starts_with(&env.root),
            "{key} escapes isolation: {value:?}"
        );
    }
}

fn konoma(args: &[&str]) -> Output {
    let env = environment::Environment::new("cli-args");
    command(args, &env)
        .supervised_output()
        .expect("start konoma")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn version_prints_the_crate_version_and_exits_0() {
    for flag in ["--version", "-V"] {
        let o = konoma(&[flag]);
        assert_eq!(o.status.code(), Some(0), "{flag}");
        assert_eq!(
            stdout(&o),
            format!("konoma {}\n", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
        assert!(o.stderr.is_empty(), "{flag}");
    }
}

#[test]
fn help_prints_the_usage_and_exits_0() {
    for flag in ["--help", "-h"] {
        let o = konoma(&[flag]);
        assert_eq!(o.status.code(), Some(0), "{flag}");
        let out = stdout(&o);
        assert!(
            out.starts_with(&format!("konoma {} ", env!("CARGO_PKG_VERSION"))),
            "{flag}: {out}"
        );
        for needle in [
            "Usage: konoma [DIR]",
            "konoma FILE",
            "--plugin PATH",
            "--plugins",
            "konoma plugin <install|update|remove|list|clean>",
            "konoma ./plugin",
            "-h, --help",
            "-V, --version",
            "Documentation: https://lesim-co-ltd.github.io/konoma/\n",
        ] {
            assert!(out.contains(needle), "{flag}: missing {needle:?}");
        }
        assert!(o.stderr.is_empty(), "{flag}");
    }
}

#[test]
fn a_directory_that_does_not_exist_fails_before_the_terminal_with_exit_1() {
    let o = konoma(&["/nonexistent/konoma-cli-args-dir"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("/nonexistent/konoma-cli-args-dir"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn plugins_lists_the_standard_plugins_and_exits_0() {
    let o = konoma(&["--plugins"]);
    assert_eq!(o.status.code(), Some(0));
    let out = stdout(&o);
    for id in ["markdown", "image", "pdf", "csv"] {
        assert!(out.contains(id), "missing {id}: {out}");
    }
}
