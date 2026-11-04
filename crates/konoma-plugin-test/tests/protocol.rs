mod support;

mod command {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/command.rs"
    ));
}
use command::CommandOutput;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use konoma_plugin::proc::client::Limits;
use konoma_plugin_test::{run, Options, Report, Status};

fn failure_limits(mode: &str) -> Limits {
    let mut limits = Limits::default();
    let short = Duration::from_millis(250);
    match mode {
        "handshake" => limits.start_timeout = short,
        "open" | "view-timeout" | "hints-timeout" => limits.open_view = short,
        "act-timeout" => limits.act_key = short,
        "cancel" => limits.cancel_grace = short,
        "shutdown" | "malformed" | "oversized" => limits.shutdown_grace = short,
        _ => {}
    }
    limits
}

fn options(mode: &str) -> Options {
    Options {
        program: std::env::current_exe().unwrap(),
        args: vec!["--fixture".into(), mode.into()],
        plugin: None,
        files: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/input.konoma-demo")],
    }
}

fn failed(mode: &str, name: &str, actual: &str) -> Report {
    let report = run(&options(mode), failure_limits(mode));
    let check = report
        .checks
        .iter()
        .find(|c| c.name.starts_with(name))
        .unwrap_or_else(|| panic!("missing {name}: {report:?}"));
    match &check.status {
        Status::Fail(reason) => {
            assert!(reason.contains("expected"), "{reason}");
            assert!(reason.contains("got"), "{reason}");
            assert!(reason.contains(actual), "{reason}");
            assert!(
                reason.contains("fixture diagnostic"),
                "missing stderr tail: {reason}"
            );
            assert!(
                !reason.contains('\n'),
                "a check must occupy one line: {reason}"
            );
        }
        status => panic!("{mode}: expected failure of {name}, got {status:?}"),
    }
    report
}

fn all_pass(report: Report) {
    assert!(report.checks.len() >= 12, "{report:?}");
    assert!(
        report.checks.iter().all(|c| c.status == Status::Pass),
        "{report:?}"
    );
}

fn handshake() {
    failed("handshake", "handshake", "deadline");
}
fn handshake_version() {
    failed("handshake-version", "handshake", "4294967295");
}
fn handshake_manifest() {
    failed("handshake-manifest", "handshake", "manifest");
}
fn manifest() {
    failed("manifest", "manifest", "ShowsNotDeclared");
}
fn manifest_keys() {
    failed("manifest-keys", "manifest", "Reserved");
}
fn manifest_id() {
    failed("manifest-id", "manifest", "the plugin id");
}
fn manifest_declaration() {
    failed("manifest-declaration", "manifest", "an action id");
}
fn manifest_glob() {
    failed("manifest-glob", "manifest", "InvalidGlob");
}
fn refusal() {
    failed("refusal", "version refusal", "Ack");
}
fn refusal_empty() {
    failed("refusal-empty", "version refusal", "empty supported");
}
fn refusal_crash() {
    failed("refusal-crash", "version refusal", "exit status");
}
fn open() {
    let r = failed("open", "open", "deadline");
    assert!(r
        .checks
        .iter()
        .any(|c| c.name == "unknown session" && c.status == Status::Pass));
    assert!(r
        .checks
        .iter()
        .any(|c| c.name == "shutdown" && c.status == Status::Pass));
}
fn view() {
    failed("view", "view", "Text");
}
fn view_timeout() {
    failed("view-timeout", "view", "deadline");
}
fn hints() {
    failed("hints", "hints", "unlabelled");
}
fn hints_eof() {
    failed("hints-eof", "hints", "EOF");
}
fn hints_timeout() {
    failed("hints-timeout", "hints", "deadline");
}
fn hints_frame() {
    failed("hints-frame", "hints", "malformed frame");
}
fn hints_wrong_reply() {
    failed("hints-wrong-reply", "hints", "Opened");
}
fn hints_wrong_rid() {
    failed("hints-wrong-rid", "hints", "answer rid");
}
fn hints_view_error() {
    let report = run(&options("hints-view-error"), Limits::default());
    let check = report
        .checks
        .iter()
        .find(|c| c.name.starts_with("hints "))
        .unwrap();
    assert_eq!(check.status, Status::Pass, "{report:?}");
}
fn act() {
    failed("act", "act", "EOF");
}
fn key() {
    failed("key", "key", "EOF");
}
fn act_exit_after_reply() {
    failed("act-exit-after-reply", "act", "EOF");
}
fn key_exit_after_reply() {
    failed("key-exit-after-reply", "key", "EOF");
}
fn act_timeout() {
    failed("act-timeout", "act", "deadline");
}
fn act_frame() {
    failed("act-frame", "act", "malformed frame");
}
fn unknown() {
    failed("unknown", "unknown session", "EOF");
}
fn unknown_ok() {
    failed("unknown-ok", "unknown session", "Ok");
}
fn cancel() {
    failed("cancel", "cancel", "deadline");
}
fn shutdown() {
    failed("shutdown", "shutdown", "deadline");
}
fn shutdown_error() {
    failed("shutdown-error", "shutdown", "exit status");
}
fn malformed() {
    failed("malformed", "malformed body", "deadline");
}
fn oversized() {
    failed("oversized", "oversized frame", "deadline");
}
fn good() {
    all_pass(run(&options("good"), Limits::default()));
}
fn relative_args() {
    let mut o = options("relative-args");
    let cwd = std::env::current_dir().unwrap();
    o.args
        .push(o.files[0].strip_prefix(cwd).unwrap().as_os_str().to_owned());
    all_pass(run(&o, Limits::default()));
}
fn action_error() {
    all_pass(run(&options("action-error"), Limits::default()));
}
fn disabled() {
    let r = run(&options("disabled"), Limits::default());
    assert!(
        r.checks
            .iter()
            .all(|c| !matches!(c.status, Status::Fail(_))),
        "{r:?}"
    );
    assert!(r
        .checks
        .iter()
        .any(|c| c.name.starts_with("act") && matches!(c.status, Status::Skip(_))));
}
fn multiple_files() {
    let mut o = options("good");
    o.files.push(o.files[0].clone());
    let r = run(&o, Limits::default());
    assert_eq!(
        r.checks
            .iter()
            .filter(|c| c.name.starts_with("open"))
            .count(),
        2
    );
    assert!(r.checks.iter().all(|c| c.status == Status::Pass), "{r:?}");
}
fn no_file() {
    let mut o = options("good");
    o.files.clear();
    let r = run(&o, Limits::default());
    assert!(r
        .checks
        .iter()
        .any(|c| c.name.starts_with("open") && matches!(c.status, Status::Skip(_))));
    assert!(
        r.checks
            .iter()
            .all(|c| !matches!(c.status, Status::Fail(_))),
        "{r:?}"
    );
    assert_eq!(r.passed(), 7);
    assert_eq!(r.failed(), 0);
    assert_eq!(r.skipped(), 6);
    assert_eq!(r.exit_code(), 0);
}
fn multi_plugin() {
    for id in ["demo-clock", "second"] {
        let mut o = options("multi");
        o.plugin = Some(id.into());
        all_pass(run(&o, Limits::default()));
        let output = Command::new(env!("CARGO_BIN_EXE_konoma-plugin-test"))
            .args(["--plugin", id, "--file"])
            .arg(&o.files[0])
            .arg(&o.program)
            .arg("--")
            .args(&o.args)
            .supervised_output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    }
}
fn clock() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = std::path::absolute(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest.parent().unwrap().parent().unwrap().join("target")),
    )
    .unwrap();
    let program = support::clock_build::build(
        std::ffi::OsStr::new(env!("CARGO")),
        manifest,
        &std::env::current_exe().unwrap(),
        &target,
        |command| command.supervised_status(),
    )
    .unwrap();
    let mut o = options("good");
    o.program = program;
    o.args.clear();
    all_pass(run(&o, Limits::default()));
    let output = Command::new(env!("CARGO_BIN_EXE_konoma-plugin-test"))
        .arg("--file")
        .arg(&o.files[0])
        .arg(&o.program)
        .supervised_output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

fn cli_short_deadline() {
    let o = options("cancel");
    let mut args = vec![
        "--file".into(),
        o.files[0].as_os_str().to_owned(),
        o.program.into_os_string(),
        "--".into(),
    ];
    args.extend(o.args);
    let mut out = Vec::new();
    let code =
        konoma_plugin_test::cli::execute(args, failure_limits("cancel"), &mut out, &mut Vec::new());
    assert_eq!(code, 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("FAIL cancel"), "{text}");
    assert!(text.contains("deadline"), "{text}");
}
fn cli() {
    let exe = env!("CARGO_BIN_EXE_konoma-plugin-test");
    for args in [
        vec![],
        vec!["--file"],
        vec!["--wat"],
        vec!["--plugin"],
        vec!["--", "x"],
        vec!["x", "extra"],
    ] {
        let out = Command::new(exe).args(args).supervised_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{out:?}");
    }
    for flag in ["--help", "--version"] {
        let out = Command::new(exe).arg(flag).supervised_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(!out.stdout.is_empty());
    }
    let out = Command::new(exe)
        .arg("/definitely-absent-konoma-plugin")
        .supervised_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    for (mode, code) in [("good", 0), ("handshake-version", 1)] {
        let o = options(mode);
        let out = Command::new(exe)
            .arg(&o.program)
            .arg("--")
            .args(&o.args)
            .supervised_output()
            .unwrap();
        assert_eq!(out.status.code(), Some(code), "{out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("SKIP open"), "{text}");
        assert!(text.contains("skipped"), "{text}");
        assert!(text.contains("failed"), "{text}");
    }
    let o = options("good");
    let out = Command::new(exe)
        .args(["--file"])
        .arg(&o.files[0])
        .arg(&o.program)
        .arg("--")
        .args(&o.args)
        .supervised_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("0 failed, 0 skipped"));
}

fn cli_signals_stop_the_plugin_group() {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::Stdio;
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let (mut ready, child_ready) = UnixStream::pair().unwrap();
        ready
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let fd = child_ready.as_raw_fd();
        let mut command = Command::new(env!("CARGO_BIN_EXE_konoma-plugin-test"));
        command
            .process_group(0)
            .arg(std::env::current_exe().unwrap())
            .args(["--", "--fixture", "signal"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: dup2 and fcntl are async-signal-safe; fd is held until spawn returns.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(fd, 3) == -1 || libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut cli = konoma_plugin::proc::process::ChildGuard::new(command.spawn().unwrap());
        drop(child_ready);
        let mut bytes = [0; 4];
        ready.read_exact(&mut bytes).unwrap();
        let pgid = i32::from_ne_bytes(bytes);
        let handoff = std::env::temp_dir().join(format!("konoma-plugin-test-{}-0", cli.id()));
        assert!(
            handoff.is_dir(),
            "the active connection owns a handoff directory"
        );
        std::fs::write(handoff.join("signal-cleanup-probe"), b"owned by this test").unwrap();
        let mut member = Command::new("/bin/cat")
            .process_group(pgid)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let input = member.stdin.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                let result = konoma_plugin::proc::process::poll_exit(&member);
                if !matches!(result, Ok(None)) {
                    tx.send(result.map(Option::unwrap)).unwrap();
                    break;
                }
                if start.elapsed() >= Duration::from_secs(3) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            member
        });
        // SAFETY: these pids identify only processes created by this test.
        unsafe {
            libc::kill(cli.id() as i32, signal);
        }
        let status = konoma_plugin::proc::process::wait(&mut cli, Duration::from_secs(10));
        let handoff_removed = !handoff.exists();
        // Clean up the exact fixture directory even when the old signal handler bypasses Drop.
        if !handoff_removed {
            std::fs::remove_dir_all(&handoff).unwrap();
        }
        let ended = rx.recv_timeout(Duration::from_secs(3));
        let mut member = waiter.join().unwrap();
        if ended.is_err() {
            // The unreaped cat (alive or zombie) still reserves this group, even if the
            // checker already reaped its leader. Reap it only after the final group signal.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        konoma_plugin::proc::process::stop_group(&mut member, Duration::ZERO).unwrap();
        drop(input);
        assert_eq!(
            ended
                .expect("CLI must stop the plugin's entire group before exiting")
                .unwrap()
                .signal(),
            Some(libc::SIGKILL)
        );
        assert!(
            handoff_removed,
            "signal exit left the handoff directory behind"
        );
        assert_eq!(status.unwrap().code(), Some(128 + signal));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--fixture") {
        return support::fixture(&args[1], &args[2..]);
    }
    let tests: &[(&str, fn())] = &[
        ("handshake", handshake),
        ("handshake_version", handshake_version),
        ("handshake_manifest", handshake_manifest),
        ("manifest", manifest),
        ("manifest_keys", manifest_keys),
        ("manifest_id", manifest_id),
        ("manifest_declaration", manifest_declaration),
        ("manifest_glob", manifest_glob),
        ("refusal", refusal),
        ("refusal_empty", refusal_empty),
        ("refusal_crash", refusal_crash),
        ("open", open),
        ("view", view),
        ("view_timeout", view_timeout),
        ("hints", hints),
        ("hints_eof", hints_eof),
        ("hints_timeout", hints_timeout),
        ("hints_frame", hints_frame),
        ("hints_wrong_reply", hints_wrong_reply),
        ("hints_wrong_rid", hints_wrong_rid),
        ("hints_view_error", hints_view_error),
        ("act", act),
        ("key", key),
        ("act_exit_after_reply", act_exit_after_reply),
        ("key_exit_after_reply", key_exit_after_reply),
        ("act_timeout", act_timeout),
        ("act_frame", act_frame),
        ("unknown", unknown),
        ("unknown_ok", unknown_ok),
        ("cancel", cancel),
        ("shutdown", shutdown),
        ("shutdown_error", shutdown_error),
        ("malformed", malformed),
        ("oversized", oversized),
        ("good", good),
        ("relative_args", relative_args),
        ("action_error", action_error),
        ("disabled", disabled),
        ("multiple_files", multiple_files),
        ("no_file", no_file),
        ("multi_plugin", multi_plugin),
        ("clock", clock),
        ("cli", cli),
        (
            "cli_signals_stop_the_plugin_group",
            cli_signals_stop_the_plugin_group,
        ),
        ("cli_short_deadline", cli_short_deadline),
    ];
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            for (name, _) in tests {
                println!("{name}: test");
            }
        }
        return ExitCode::SUCCESS;
    }
    let filter = args
        .iter()
        .position(|a| a == "--exact")
        .and_then(|i| args.get(i + 1));
    let mut count = 0;
    let mut failures = 0;
    for (name, test) in tests {
        if filter.is_some_and(|f| f != name) {
            continue;
        }
        count += 1;
        if std::panic::catch_unwind(*test).is_err() {
            failures += 1;
            println!("test {name} ... FAILED");
        } else {
            println!("test {name} ... ok");
        }
    }
    if failures == 0 && count > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}
