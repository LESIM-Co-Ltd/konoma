//! The `konoma` command's arguments, through the real binary: what the library split
//! (`konoma::Host` + a short `main`) must not change. None of these touch a terminal.

use std::process::{Command, Output};

fn konoma(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_konoma"))
        .args(args)
        .env("HOME", std::env::temp_dir().join("konoma-cli-args-home"))
        .stdin(std::process::Stdio::null())
        .output()
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
