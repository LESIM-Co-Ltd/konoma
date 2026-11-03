//! Small OS-string parser: plugin arguments after `--` are passed through byte for byte.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{run, Limits, Options, Status};

const HELP: &str = "Usage: konoma-plugin-test [OPTIONS] <PROGRAM> [-- <ARG>...]\n\nOptions:\n  --plugin <ID>   Select a plugin in a multi-plugin program\n  --file <PATH>   File to open (repeatable); omitted checks are reported as skipped\n  --help          Show this help\n  --version       Show version\n\nUses konoma's production deadlines. Child arguments precede --plugin <ID> --serve.\n";

enum Parsed {
    Run(Options),
    Help,
    Version,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    let mut it = args.into_iter();
    let mut program = None;
    let mut plugin = None;
    let mut files = Vec::new();
    let mut child_args = Vec::new();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--help") => return Ok(Parsed::Help),
            Some("--version") => return Ok(Parsed::Version),
            Some("--plugin") => {
                let id = it.next().ok_or("--plugin requires an ID")?;
                let id = id.into_string().map_err(|_| "plugin ID must be UTF-8")?;
                if id.is_empty() || id.starts_with('-') {
                    return Err("--plugin requires an ID".into());
                }
                if plugin.replace(id).is_some() {
                    return Err("--plugin may only be supplied once".into());
                }
            }
            Some("--file") => {
                let file = it.next().ok_or("--file requires a PATH")?;
                if file.is_empty() || file.to_string_lossy().starts_with("--") {
                    return Err("--file requires a PATH".into());
                }
                files.push(PathBuf::from(file));
            }
            Some("--") => {
                if program.is_none() {
                    return Err("PROGRAM is required before --".into());
                }
                child_args.extend(it);
                break;
            }
            _ if arg.to_string_lossy().starts_with('-') => {
                return Err(format!("unknown option {:?}", arg))
            }
            _ => {
                if program.replace(PathBuf::from(arg)).is_some() {
                    return Err("put program arguments after --".into());
                }
            }
        }
    }
    let program = resolve(&program.ok_or("PROGRAM is required")?)?;
    for file in &files {
        if !file.is_file() {
            return Err(format!(
                "file does not exist or is not a regular file: {}",
                file.display()
            ));
        }
    }
    Ok(Parsed::Run(Options {
        program,
        plugin,
        files,
        args: child_args,
    }))
}

fn resolve(program: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    let path = if program.components().count() > 1 || program.is_absolute() {
        executable(program).then(|| program.to_path_buf())
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(program))
                .find(|p| executable(p))
        })
    };
    path.and_then(|p| std::path::absolute(p).ok())
        .ok_or_else(|| format!("program not found or not executable: {}", program.display()))
}

/// Execute the CLI with explicit limits and streams. The executable passes `Limits::default()`;
/// embedding tests can use short deadlines without introducing a user-visible timeout option.
pub fn execute(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    limits: Limits,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let args = args.into_iter().map(|a| a.as_ref().to_os_string());
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = writeln!(err, "error: {e}\nUse --help for usage.");
            return 2;
        }
    };
    let result = match parsed {
        Parsed::Help => write!(out, "{HELP}").map(|()| 0),
        Parsed::Version => {
            writeln!(out, "konoma-plugin-test {}", env!("CARGO_PKG_VERSION")).map(|()| 0)
        }
        Parsed::Run(options) => {
            let report = run(&options, limits);
            let written = (|| {
                for check in &report.checks {
                    match &check.status {
                        Status::Pass => writeln!(out, "PASS {}", check.name)?,
                        Status::Fail(reason) => writeln!(out, "FAIL {}: {reason}", check.name)?,
                        Status::Skip(reason) => writeln!(out, "SKIP {}: {reason}", check.name)?,
                    }
                }
                writeln!(
                    out,
                    "{} passed, {} failed, {} skipped",
                    report.passed(),
                    report.failed(),
                    report.skipped()
                )
            })();
            written.map(|()| report.exit_code())
        }
    };
    result.unwrap_or(1)
}
