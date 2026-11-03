//! `konoma plugin install | update | remove | list | clean` and the hand-over to the rebuilt
//! konoma, through the real binary. Cargo is a script (`$CARGO`) that writes a fake program, so
//! no test here builds anything or touches the network; one `#[ignore]` test runs the real cargo.
//!
//! HOME, XDG_CONFIG_HOME and XDG_DATA_HOME are directories of the test, never the user's.

#[path = "support/command.rs"]
mod command;
use command::CommandOutput;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use konoma_test_support::{unique_tmp, TmpDir};

const OWN: &str = env!("CARGO_PKG_VERSION");

const FAKE_CARGO: &str = r##"#!/bin/sh
echo "$*" >> "$FAKE_DIR/cargo.log"
case "$1" in
metadata)
  if [ -n "$FAKE_METADATA_READY" ]; then
    sleep 300 &
    printf '%s %s\n' "$$" "$!" > "$FAKE_METADATA_READY"
    IFS= read -r continue_metadata < "$FAKE_METADATA_RELEASE"
    test -f Cargo.toml || exit 71
  fi
  if [ -n "$FAKE_METADATA_FAIL" ]; then echo 'fixture metadata failure' >&2; exit 72; fi
  authors='["Fixture author"]'
  if [ "$FAKE_METADATA_NAME" = konoma-bar ]; then authors='[]'; fi
  printf '{"packages":[{"id":"fixture","name":"%s","authors":%s}],"resolve":{"root":"root","nodes":[{"id":"root","deps":[{"pkg":"fixture"}]}]}}\n' "$FAKE_METADATA_NAME" "$authors"
  exit 0 ;;
update)
  if [ -n "$FAKE_LOCK_NEW" ] && [ -f Cargo.lock ]; then cp "$FAKE_LOCK_NEW" Cargo.lock; fi
  exit 0 ;;
build)
  if [ -f Cargo.lock ]; then echo "lock-present" >> "$FAKE_DIR/cargo.log"; else cp "$FAKE_DIR/lock-initial" Cargo.lock; fi
  cp Cargo.toml "$FAKE_DIR/last-Cargo.toml"
  cp src/main.rs "$FAKE_DIR/last-main.rs"
  if [ -n "$FAKE_GRANDCHILD" ]; then
    # like a build tool that passes the signal on to the job it started
    # (sent to the job's pid, not the group: `kill -- -$$` fails in dash)
    gc=
    if [ -n "$FAKE_TERM_KILLS_GROUP" ]; then trap 'trap "" TERM; [ -n "$gc" ] && kill -TERM "$gc"; exit 1' TERM; fi
    # a process of its own, as rustc is to cargo; its pid goes to the file
    if [ -n "$FAKE_GC_IGNORE_TERM" ]; then (trap '' TERM; exec sleep 300) & else sleep 300 & fi
    gc=$!
    echo $gc > "$FAKE_GRANDCHILD"
    touch "$FAKE_MARK"
    wait
    exit 1
  fi
  if [ -n "$FAKE_MARK" ]; then touch "$FAKE_MARK"; exec sleep 30; fi
  if [ -n "$FAKE_FAIL_BUILD" ]; then echo "error: fake build failure" >&2; exit 101; fi
  n=$(cat "$FAKE_DIR/counter" 2>/dev/null || echo 0)
  n=$((n+1))
  echo $n > "$FAKE_DIR/counter"
  mkdir -p "$CARGO_TARGET_DIR/release"
  cp "$FAKE_DIR/fake-konoma" "$CARGO_TARGET_DIR/release/konoma"
  echo "# build $n" >> "$CARGO_TARGET_DIR/release/konoma"
  chmod +x "$CARGO_TARGET_DIR/release/konoma"
  exit 0 ;;
esac
exit 0
"##;

const FAKE_KONOMA: &str = r#"#!/bin/sh
case "$1" in
--plugins)
  if [ -n "$FAKE_VERIFY_FAIL" ]; then echo "duplicate plugin id: markdown" >&2; exit 1; fi
  if [ -n "$FAKE_VERIFY_MARK" ]; then echo $$ > "$FAKE_VERIFY_MARK.tmp"; mv "$FAKE_VERIFY_MARK.tmp" "$FAKE_VERIFY_MARK"; exec sleep 30; fi
  echo "fake plugin table"; exit 0 ;;
--version)
  echo "fake-konoma from=$KONOMA_PLUGIN_SWITCHED_FROM"; exit 0 ;;
*)
  echo "fake-ran: $* switched=$KONOMA_PLUGIN_SWITCHED_FROM"; exit 0 ;;
esac
"#;

struct Sandbox {
    root: TmpDir,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = unique_tmp(name);
        let s = Sandbox { root };
        for d in [
            s.home(),
            s.config(),
            s.data(),
            s.fake(),
            s.root.join("tmp"),
            s.root.join("cache"),
            s.root.join("runtime"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        write_exe(&s.fake().join("cargo"), FAKE_CARGO);
        write_exe(&s.fake().join("fake-konoma"), FAKE_KONOMA);
        std::fs::create_dir_all(s.config().join("konoma")).unwrap();
        std::fs::write(s.config().join("konoma/config.toml"), "[ui]\nlang = 'en'\n").unwrap();
        std::fs::write(
            s.fake().join("lock-initial"),
            format!(
                "version = 4\n\n[[package]]\nname = \"konoma-with-plugins\"\nversion = \"0.0.0\"\ndependencies = [\"konoma\", \"konoma-foo\", \"konoma-bar\"]\n\n[[package]]\nname = \"konoma\"\nversion = \"{OWN}\"\n\n\
                 [[package]]\nname = \"konoma-foo\"\nversion = \"0.3.2\"\n\n\
                 [[package]]\nname = \"konoma-bar\"\nversion = \"1.0.0\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            s.fake().join("lock-new"),
            format!(
                "version = 4\n\n[[package]]\nname = \"konoma-with-plugins\"\nversion = \"0.0.0\"\ndependencies = [\"konoma\", \"konoma-foo\", \"konoma-bar\"]\n\n[[package]]\nname = \"konoma\"\nversion = \"{OWN}\"\n\n\
                 [[package]]\nname = \"konoma-foo\"\nversion = \"0.3.5\"\n\n\
                 [[package]]\nname = \"konoma-bar\"\nversion = \"1.0.0\"\n"
            ),
        )
        .unwrap();
        s
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn config(&self) -> PathBuf {
        self.root.join("xdg-config")
    }
    fn data_base(&self) -> PathBuf {
        self.root.join("xdg-data")
    }
    fn data(&self) -> PathBuf {
        self.data_base().join("konoma")
    }
    fn fake(&self) -> PathBuf {
        self.root.join("fake")
    }
    fn plugins_toml(&self) -> PathBuf {
        self.config().join("konoma").join("plugins.toml")
    }
    fn program(&self) -> PathBuf {
        self.data().join("bin").join("konoma")
    }
    fn record(&self) -> PathBuf {
        self.data().join("bin").join("konoma.built.toml")
    }
    fn lock(&self) -> PathBuf {
        self.data().join("Cargo.lock")
    }
    fn cargo_log(&self) -> String {
        std::fs::read_to_string(self.fake().join("cargo.log")).unwrap_or_default()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_konoma"));
        c.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config())
            .env("XDG_DATA_HOME", self.data_base())
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("CARGO", self.fake().join("cargo"))
            .env("FAKE_DIR", self.fake())
            .env(
                "FAKE_METADATA_NAME",
                args.get(2).unwrap_or(&"").split('@').next().unwrap(),
            )
            .env("FAKE_LOCK_NEW", self.fake().join("lock-new"))
            .current_dir(self.home())
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args)
            .supervised_output()
            .expect("start konoma")
    }

    fn run_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = self.command(args);
        for (k, v) in envs {
            c.env(k, v);
        }
        c.supervised_output().expect("start konoma")
    }

    /// The work directories a run leaves in the data directory (`build`, `build-<pid>`).
    fn leftovers(&self) -> Vec<String> {
        let mut found = Vec::new();
        for (dir, label) in [
            (self.data(), "data"),
            (self.data().join("bin"), "bin"),
            (
                self.plugins_toml().parent().unwrap().to_path_buf(),
                "config",
            ),
        ] {
            let rd = match std::fs::read_dir(dir) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => panic!("{e}"),
            };
            for e in rd {
                let name = e.unwrap().file_name().to_string_lossy().into_owned();
                if name == "build"
                    || name.starts_with("build-")
                    || name.starts_with(".konoma.new-")
                    || (name.starts_with('.') && name.contains(".tmp-"))
                {
                    found.push(format!("{label}/{name}"));
                }
            }
        }
        found.sort();
        found
    }
}

fn write_exe(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

#[test]
fn leftovers_include_all_atomic_and_staged_file_directories() {
    let s = Sandbox::new("plugin-all-leftovers");
    for (dir, name) in [
        (s.data(), ".Cargo.lock.tmp-fixture"),
        (s.data().join("bin"), ".konoma.new-fixture"),
        (s.data().join("bin"), ".konoma.built.toml.tmp-fixture"),
        (
            s.plugins_toml().parent().unwrap().to_path_buf(),
            ".plugins.toml.tmp-fixture",
        ),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"partial").unwrap();
    }
    assert_eq!(s.leftovers().len(), 4, "{:?}", s.leftovers());
}

struct MetadataGate {
    ready: File,
    release: File,
    ready_path: PathBuf,
    release_path: PathBuf,
}

impl MetadataGate {
    fn new(s: &Sandbox) -> Self {
        use std::os::unix::fs::OpenOptionsExt;
        let ready_path = s.root.join("metadata-ready");
        let release_path = s.root.join("metadata-release");
        let open = |path: &Path| {
            let cpath = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
            // SAFETY: the FIFO belongs to this test and the C string is valid.
            assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
            File::options()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
                .unwrap()
        };
        Self {
            ready: open(&ready_path),
            release: open(&release_path),
            ready_path,
            release_path,
        }
    }

    fn configure(&self, command: &mut Command) {
        command
            .env("FAKE_METADATA_READY", &self.ready_path)
            .env("FAKE_METADATA_RELEASE", &self.release_path);
    }

    fn entered(&mut self) -> (u32, u32) {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "metadata readiness deadline");
            let mut event = libc::pollfd {
                fd: self.ready.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives a valid single descriptor record and a bounded watchdog.
            assert!(unsafe { libc::poll(&mut event, 1, 100) } >= 0);
            let mut buf = [0; 128];
            match self.ready.read(&mut buf) {
                Ok(n) => line.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            }
            if line.contains(&b'\n') {
                let text = String::from_utf8(line).unwrap();
                let mut pids = text.split_whitespace().map(|s| s.parse().unwrap());
                return (pids.next().unwrap(), pids.next().unwrap());
            }
        }
    }

    fn release(&mut self) {
        use std::io::Write;
        self.release.write_all(b"continue\n").unwrap();
    }
}

fn metadata_signal_stops_cargo(sig: i32) {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    let s = Sandbox::new("metadata-signal");
    let mut gate = MetadataGate::new(&s);
    let (mut lifetime, child_lifetime) = UnixStream::pair().unwrap();
    lifetime
        .set_read_timeout(Some(Duration::from_secs(6)))
        .unwrap();
    let fd = child_lifetime.as_raw_fd();
    let mut command = s.command(&["plugin", "install", "konoma-foo", "--yes"]);
    gate.configure(&mut command);
    command
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: these descriptor operations are async-signal-safe; fd stays live through spawn.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 3) == -1 || libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut cli = command.spawn().unwrap();
    let _cli_guard = KillOnDrop(cli.id());
    drop(child_lifetime);
    let (cargo, grandchild) = gate.entered();
    let _cargo_guard = KillOnDrop(cargo);
    let _grandchild_guard = KillOnDrop(grandchild);
    // SAFETY: both pids were recorded by the fake cargo created by this test.
    let group = unsafe { libc::getpgid(cargo as i32) };
    assert!(group > 0);
    unsafe {
        assert_eq!(libc::kill(cli.id() as i32, sig), 0);
    }
    let status = wait_exit(&mut cli, 10, "metadata interruption");
    let mut byte = [0];
    let ended = lifetime.read(&mut byte);
    if !matches!(ended, Ok(0)) {
        // Stop the exact owned group on the old implementation before reporting failure.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    assert!(
        matches!(ended, Ok(0)),
        "metadata must stop cargo and every child holding the lifetime pipe: {ended:?}"
    );
    assert_eq!(status.code(), Some(130));
    assert!(
        s.leftovers().is_empty(),
        "metadata work directory must be removed"
    );
    assert!(!s.program().exists() && !s.plugins_toml().exists());
    // A direct cargo child is reaped before konoma returns, rather than left as a zombie.
    assert_eq!(unsafe { libc::kill(cargo as i32, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn metadata_sigint_stops_cargo_and_its_children() {
    metadata_signal_stops_cargo(libc::SIGINT);
}

#[test]
fn metadata_sigterm_stops_cargo_and_its_children() {
    metadata_signal_stops_cargo(libc::SIGTERM);
}

#[test]
fn clean_waits_for_an_in_use_metadata_directory() {
    use std::io::BufRead;
    let s = Sandbox::new("metadata-clean-lock");
    let mut gate = MetadataGate::new(&s);
    let mut command = s.command(&["plugin", "install", "konoma-foo", "--yes"]);
    gate.configure(&mut command);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut install = command.spawn().unwrap();
    let _install_guard = KillOnDrop(install.id());
    let (cargo, grandchild) = gate.entered();
    let _cargo_guard = KillOnDrop(cargo);
    let _grandchild_guard = KillOnDrop(grandchild);
    let work = s.data().join(format!("build-{}", install.id()));
    assert!(work.join("Cargo.toml").is_file());
    let mut clean = s
        .command(&["plugin", "clean"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _clean_guard = KillOnDrop(clean.id());
    let stderr = clean.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        std::io::BufReader::new(stderr)
            .read_line(&mut line)
            .unwrap();
        tx.send(line).unwrap();
    });
    let attempted = rx.recv_timeout(Duration::from_secs(6));
    let survived = work.join("Cargo.toml").is_file();
    let waiting = clean.try_wait().unwrap().is_none();
    gate.release();
    // The fake cargo's child is only needed by the signal checks; end it before waiting on output.
    unsafe {
        libc::kill(grandchild as i32, libc::SIGKILL);
    }
    let install_status = wait_exit(&mut install, 15, "install after metadata release");
    let clean_status = wait_exit(&mut clean, 15, "clean after metadata release");
    reader.join().unwrap();
    assert!(
        attempted.unwrap().contains("waiting"),
        "clean must attempt the shared lock"
    );
    assert!(
        survived && waiting,
        "clean deleted an active metadata work directory"
    );
    assert!(install_status.success() && clean_status.success());
}

#[test]
fn install_path_resolves_symlinks_before_parent_components() {
    let s = Sandbox::new("plugin-path-links");
    std::fs::create_dir_all(s.root.join("sdk/subdir")).unwrap();
    std::fs::create_dir_all(s.root.join("sdk/plugin")).unwrap();
    std::os::unix::fs::symlink(s.root.join("sdk/subdir"), s.home().join("link")).unwrap();
    let o = s.run(&[
        "plugin",
        "install",
        "konoma-foo",
        "--path",
        "link/../plugin",
        "--yes",
    ]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let saved = std::fs::read_to_string(s.plugins_toml()).unwrap();
    let expected = s.root.join("sdk/plugin").canonicalize().unwrap();
    assert!(saved.contains(expected.to_str().unwrap()), "{saved}");
}

#[test]
fn install_path_must_exist_before_cargo_runs() {
    let s = Sandbox::new("plugin-path-missing");
    let o = s.run(&[
        "plugin",
        "install",
        "konoma-foo",
        "--path",
        "missing",
        "--yes",
    ]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(s.cargo_log().is_empty());
    assert!(!s.plugins_toml().exists());
}

#[test]
fn japanese_missing_path_uses_the_cli_language() {
    let s = Sandbox::new("plugin-japanese-path");
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(
        s.config().join("konoma/config.toml"),
        "[ui]\nlang = \"ja\"\n",
    )
    .unwrap();
    let o = s.run(&[
        "plugin",
        "install",
        "konoma-foo",
        "--path",
        "missing",
        "--yes",
    ]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("実体を解決できません"), "{}", err(&o));
}

#[test]
fn japanese_lock_restore_error_uses_the_cli_language() {
    let s = Sandbox::new("plugin-japanese-lock");
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(
        s.config().join("konoma/config.toml"),
        "[ui]\nlang = \"ja\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(s.lock()).unwrap();
    let o = s.run(&["plugin", "install", "konoma-foo", "--yes"]);
    assert_eq!(code(&o), 1);
    assert!(
        err(&o).contains("保存した Cargo.lock を復元できません"),
        "{}",
        err(&o)
    );
}

#[test]
fn japanese_metadata_error_uses_the_cli_language() {
    let s = Sandbox::new("plugin-japanese-metadata");
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(
        s.config().join("konoma/config.toml"),
        "[ui]\nlang = \"ja\"\n",
    )
    .unwrap();
    let o = s.run_env(
        &["plugin", "install", "konoma-foo", "--yes"],
        &[("FAKE_METADATA_FAIL", "1")],
    );
    assert_eq!(code(&o), 1);
    assert!(
        err(&o).contains("プラグインのメタデータを読めませんでした"),
        "{}",
        err(&o)
    );
}

#[test]
fn standard_only_update_rebuilds_all() {
    assert_standard_update(&["plugin", "update"], "update");
}

#[test]
fn standard_only_update_rebuilds_a_named_standard_crate() {
    assert_standard_update(
        &["plugin", "update", "konoma-office"],
        "update -p konoma-office",
    );
}

fn assert_standard_update(args: &[&str], update: &str) {
    let s = Sandbox::new("plugin-standard-update");
    let o = s.run(args);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let log = s.cargo_log();
    assert!(log.lines().any(|l| l == update), "{log}");
    assert!(log.contains("build --release"), "{log}");
    assert!(s.program().is_file());
}

#[test]
fn built_record_contains_resolved_plugin_versions() {
    let s = Sandbox::new("plugin-record-versions");
    let o = s.run(&["plugin", "install", "konoma-foo", "--yes"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let record: toml::Table = std::fs::read_to_string(s.record())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        record["plugins"].get("konoma-foo").and_then(|v| v.as_str()),
        Some("0.3.2")
    );
}

#[test]
fn install_confirmation_shows_resolved_authors_before_building() {
    let s = Sandbox::new("plugin-authors");
    let o = s.run(&["plugin", "install", "konoma-foo", "--yes"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(
        out(&o).contains("Authors:       Fixture author"),
        "{}",
        out(&o)
    );
    let log = s.cargo_log();
    assert!(
        log.find("metadata").unwrap() < log.find("build --release").unwrap(),
        "{log}"
    );
    let o = s.run(&["plugin", "install", "konoma-bar", "--yes"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("Authors:       not listed"), "{}", out(&o));
    std::fs::write(
        s.config().join("konoma/config.toml"),
        "[ui]\nlang = \"ja\"\n",
    )
    .unwrap();
    let o = s.run(&["plugin", "install", "konoma-bar", "--yes"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("作者:          記載なし"), "{}", out(&o));
}

#[test]
fn plugin_list_reads_only_the_saved_timestamp_without_changing_it() {
    let s = Sandbox::new("plugin-cli-update-list");
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(
        s.plugins_toml(),
        "[plugins.konoma-foo]\nversion = \"*\"\n[plugins.konoma-bar]\nversion = \"*\"\n",
    )
    .unwrap();
    std::fs::copy(s.fake().join("lock-initial"), s.lock()).unwrap();
    let plain = s.run(&["plugin", "list"]);
    assert_eq!(code(&plain), 0, "{}", err(&plain));
    assert!(!out(&plain).contains("Last checked:"));
    let record = "checked_at = 100\nhost_version = \"0.1.0\"\n[plugins.konoma-foo]\ninstalled = \"0.3.2\"\n[plugins.konoma-foo.updates]\ncompatible = \"0.4.0\"\nneeds_host = \"1.0.0\"\n[plugins.konoma-bar]\ninstalled = \"1.0.0\"\n[plugins.konoma-bar.updates]\nneeds_host = \"2.0.0\"\n";
    let path = s.data().join("plugin-updates.toml");
    std::fs::write(&path, record).unwrap();
    let o = s.run(&["plugin", "list"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let text = out(&o);
    assert!(!text.contains("update available:"), "{text}");
    assert!(!text.contains("needs a newer konoma"), "{text}");
    assert!(!text.contains("newer 1.0.0 needs"), "{text}");
    assert!(
        text.contains("Last checked: 1970-01-01 00:01:40 UTC"),
        "{text}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), record);
    assert!(s.cargo_log().is_empty());
    std::fs::write(&path, "broken = [").unwrap();
    let o = s.run(&["plugin", "list"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o), out(&plain));
}

#[test]
fn install_list_update_remove_cycle() {
    let s = Sandbox::new("plugin-cli-cycle");

    let o = s.run(&["plugin", "install", "konoma-foo@0.3", "--yes"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(s.program().is_file(), "the rebuilt program is in place");
    let record = std::fs::read_to_string(s.record()).unwrap();
    assert!(
        record.contains("konoma-foo") && record.contains(OWN),
        "{record}"
    );
    let toml = std::fs::read_to_string(s.plugins_toml()).unwrap();
    assert!(
        toml.contains("[plugins.konoma-foo]") && toml.contains("version = \"0.3\""),
        "{toml}"
    );
    assert!(s.lock().is_file(), "Cargo.lock is kept in the data dir");
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
    let log = s.cargo_log();
    assert!(log.contains("build --release"), "{log}");
    assert!(
        !log.contains("update"),
        "install does not bump versions: {log}"
    );
    let main_rs = std::fs::read_to_string(s.fake().join("last-main.rs")).unwrap();
    assert!(
        main_rs.contains(".plugins(konoma_foo::plugins())"),
        "{main_rs}"
    );
    let cargo_toml = std::fs::read_to_string(s.fake().join("last-Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains(&format!("version = \"={OWN}\"")),
        "{cargo_toml}"
    );

    let o = s.run(&["plugin", "list"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let listing = out(&o);
    assert!(
        listing.contains("konoma-foo") && listing.contains("0.3.2") && listing.contains("0.3"),
        "{listing}"
    );

    // update (all): cargo update, then build; plugins.toml is not rewritten
    let before = std::fs::read(s.plugins_toml()).unwrap();
    let o = s.run(&["plugin", "update"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(
        s.cargo_log().lines().any(|l| l == "update"),
        "{}",
        s.cargo_log()
    );
    assert_eq!(std::fs::read(s.plugins_toml()).unwrap(), before);
    let o = s.run(&["plugin", "list"]);
    assert!(out(&o).contains("0.3.5"), "{}", out(&o));

    let o = s.run(&["plugin", "update", "konoma-foo"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(
        s.cargo_log().lines().any(|l| l == "update -p konoma-foo"),
        "{}",
        s.cargo_log()
    );
    let o = s.run(&["plugin", "update", "konoma-nope"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("konoma-nope"), "{}", err(&o));

    // remove the last one: back to plain konoma, nothing rebuilt
    let builds = s.cargo_log().matches("build --release").count();
    let o = s.run(&["plugin", "remove", "konoma-foo"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(!s.program().exists());
    assert!(!s.record().exists());
    assert!(!s.lock().exists());
    assert_eq!(s.cargo_log().matches("build --release").count(), builds);
    let toml = std::fs::read_to_string(s.plugins_toml()).unwrap();
    assert!(!toml.contains("konoma-foo"), "{toml}");
    let o = s.run(&["plugin", "list"]);
    assert!(out(&o).contains("standard build"), "{}", out(&o));
    assert!(s.leftovers().is_empty());
}

#[test]
fn a_second_install_reuses_the_saved_lock_and_remove_rebuilds_the_rest() {
    let s = Sandbox::new("plugin-cli-two");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    assert!(!s.cargo_log().contains("lock-present"));
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-bar", "-y"])), 0);
    assert!(
        s.cargo_log().contains("lock-present"),
        "the second build starts from the saved lock: {}",
        s.cargo_log()
    );
    let toml = std::fs::read_to_string(s.plugins_toml()).unwrap();
    assert!(
        toml.contains("konoma-foo") && toml.contains("konoma-bar"),
        "{toml}"
    );
    let main_rs = std::fs::read_to_string(s.fake().join("last-main.rs")).unwrap();
    assert!(main_rs.contains("konoma_foo::plugins()") && main_rs.contains("konoma_bar::plugins()"));

    let o = s.run(&["plugin", "remove", "konoma-foo"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(s.program().is_file(), "one plugin is left: rebuilt");
    let record = std::fs::read_to_string(s.record()).unwrap();
    assert!(
        record.contains("konoma-bar") && !record.contains("konoma-foo"),
        "{record}"
    );
    let main_rs = std::fs::read_to_string(s.fake().join("last-main.rs")).unwrap();
    assert!(!main_rs.contains("konoma_foo"), "{main_rs}");

    let o = s.run(&["plugin", "remove", "konoma-foo"]);
    assert_eq!(code(&o), 1, "removing what is not installed fails");
}

#[test]
fn a_failed_build_keeps_the_old_program_and_the_list() {
    let s = Sandbox::new("plugin-cli-buildfail");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    let program = std::fs::read(s.program()).unwrap();
    let list = std::fs::read(s.plugins_toml()).unwrap();
    let lock = std::fs::read(s.lock()).unwrap();

    let o = s.run_env(
        &["plugin", "install", "konoma-bar", "-y"],
        &[("FAKE_FAIL_BUILD", "1")],
    );
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("failed"), "{}", err(&o));
    assert_eq!(std::fs::read(s.program()).unwrap(), program, "not swapped");
    assert_eq!(
        std::fs::read(s.plugins_toml()).unwrap(),
        list,
        "list not changed"
    );
    assert_eq!(std::fs::read(s.lock()).unwrap(), lock, "lock not changed");
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

#[test]
fn a_new_program_that_refuses_to_start_is_not_swapped_in() {
    let s = Sandbox::new("plugin-cli-verifyfail");
    // first install: nothing to keep
    let o = s.run_env(
        &["plugin", "install", "konoma-foo", "-y"],
        &[("FAKE_VERIFY_FAIL", "1")],
    );
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("duplicate plugin id"), "{}", err(&o));
    assert!(err(&o).contains("nothing was replaced"), "{}", err(&o));
    assert!(!s.program().exists() && !s.record().exists());
    assert!(!s.plugins_toml().exists(), "the list is not written either");
    assert!(!s.lock().exists());
    assert!(s.leftovers().is_empty());

    // with a program in place, it stays
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    let program = std::fs::read(s.program()).unwrap();
    let o = s.run_env(&["plugin", "update"], &[("FAKE_VERIFY_FAIL", "1")]);
    assert_eq!(code(&o), 1);
    assert_eq!(std::fs::read(s.program()).unwrap(), program);
    assert!(s.leftovers().is_empty());
}

#[test]
fn without_cargo_install_and_update_say_so_and_fail() {
    let s = Sandbox::new("plugin-cli-nocargo");
    let empty = s.root.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let nothing = empty.to_string_lossy().into_owned();
    for args in [
        &["plugin", "install", "konoma-foo", "-y"][..],
        &["plugin", "update"][..],
    ] {
        let mut c = s.command(args);
        c.env_remove("CARGO").env("PATH", &nothing);
        let o = c.supervised_output().unwrap();
        assert_ne!(code(&o), 0);
        let e = err(&o);
        assert!(e.contains("cargo was not found"), "{e}");
        assert!(
            e.contains("command ="),
            "the standalone-app alternative: {e}"
        );
        assert!(!s.program().exists());
        assert!(!s.plugins_toml().exists());
    }
    // with a plugin listed, update needs cargo
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(
        s.plugins_toml(),
        "[plugins.konoma-foo]\nversion = \"0.3\"\n",
    )
    .unwrap();
    let mut c = s.command(&["plugin", "update"]);
    c.env_remove("CARGO").env("PATH", &nothing);
    let o = c.supervised_output().unwrap();
    assert_ne!(code(&o), 0);
    assert!(err(&o).contains("cargo was not found"), "{}", err(&o));
    // a $CARGO that does not exist is not silently replaced by PATH
    let o = s.run_env(
        &["plugin", "update"],
        &[("CARGO", "/nonexistent/konoma-test-cargo")],
    );
    assert_ne!(code(&o), 0);
}

#[test]
fn install_without_a_terminal_needs_yes() {
    let s = Sandbox::new("plugin-cli-confirm");
    let o = s.run(&["plugin", "install", "konoma-foo"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("--yes"), "{}", err(&o));
    assert!(
        out(&o).contains("konoma-foo"),
        "name and origin are shown first: {}",
        out(&o)
    );
    assert!(s.cargo_log().is_empty(), "nothing was built");
    assert!(!s.plugins_toml().exists());
    assert!(!s.program().exists());
}

#[test]
fn the_standard_crates_are_refused() {
    let s = Sandbox::new("plugin-cli-standard");
    for c in ["konoma-code", "konoma-office", "konoma", "konoma-plugin"] {
        let o = s.run(&["plugin", "install", c, "--yes"]);
        assert_eq!(code(&o), 1, "{c}");
        assert!(err(&o).contains("standard"), "{c}: {}", err(&o));
    }
    assert!(s.cargo_log().is_empty());
}

#[test]
fn a_relative_path_is_written_absolute() {
    let s = Sandbox::new("plugin-cli-path");
    std::fs::create_dir_all(s.home().join("rel/baz")).unwrap();
    let o = s.run(&[
        "plugin",
        "install",
        "konoma-baz",
        "--path",
        "./rel/baz",
        "-y",
    ]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    let toml = std::fs::read_to_string(s.plugins_toml()).unwrap();
    let want = s.home().canonicalize().unwrap().join("rel/baz");
    assert!(
        toml.contains(&format!("path = \"{}\"", want.display())),
        "{toml}"
    );
    let o = s.run(&["plugin", "list"]);
    assert!(out(&o).contains("no update check"), "{}", out(&o));
    let cargo_toml = std::fs::read_to_string(s.fake().join("last-Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains(&want.display().to_string()),
        "{cargo_toml}"
    );
}

#[test]
fn git_installs_are_listed_without_an_update_check() {
    let s = Sandbox::new("plugin-cli-git");
    let o = s.run(&[
        "plugin",
        "install",
        "konoma-qux",
        "--git",
        "https://example.org/qux",
        "--branch",
        "dev",
        "-y",
    ]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    let o = s.run(&["plugin", "list"]);
    let l = out(&o);
    assert!(
        l.contains("https://example.org/qux") && l.contains("no update check"),
        "{l}"
    );
}

#[test]
fn plugin_without_a_command_prints_usage_and_exits_2() {
    let s = Sandbox::new("plugin-cli-usage");
    let o = s.run(&["plugin"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("Usage: konoma plugin"), "{}", err(&o));
    assert!(
        !err(&o).contains("konoma ./plugin"),
        "no hint without an entry here"
    );

    let o = s.run(&["plugin", "frobnicate"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("frobnicate"), "{}", err(&o));

    std::fs::create_dir_all(s.home().join("plugin")).unwrap();
    let o = s.run(&["plugin"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("konoma ./plugin"), "{}", err(&o));

    let o = s.run(&["plugin", "--help"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("Usage: konoma plugin"));
    let o = s.run(&["plugin", "install"]);
    assert_eq!(code(&o), 2);
    assert!(s.cargo_log().is_empty());
}

#[test]
fn a_broken_plugins_toml_is_an_error_and_is_left_alone() {
    let s = Sandbox::new("plugin-cli-broken");
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    for broken in [
        "this is = = not toml",
        "[plugins.konoma-foo]\npath = \"relative/dir\"\n",
        "[plugins.konoma-foo]\nversion = \"1\"\ngit = \"u\"\n",
        "[plugins.konoma-foo]\nwhatever = 1\n",
    ] {
        std::fs::write(s.plugins_toml(), broken).unwrap();
        for args in [
            &["plugin", "list"][..],
            &["plugin", "install", "konoma-bar", "-y"][..],
            &["plugin", "remove", "konoma-foo"][..],
            &["plugin", "update"][..],
        ] {
            let o = s.run(args);
            assert_eq!(code(&o), 1, "{broken:?} {args:?}: {}", err(&o));
            assert!(err(&o).contains("plugins.toml"), "{broken:?}: {}", err(&o));
            assert_eq!(
                std::fs::read_to_string(s.plugins_toml()).unwrap(),
                broken,
                "never rewritten"
            );
        }
    }
    assert!(s.cargo_log().is_empty(), "no build was started");
    assert!(!s.program().exists());
}

#[test]
fn konoma_hands_over_to_the_rebuilt_program_with_its_arguments() {
    let s = Sandbox::new("plugin-cli-handover");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);

    let o = s.run(&["--version"]);
    assert_eq!(code(&o), 0);
    assert_eq!(out(&o).trim(), format!("fake-konoma from={OWN}"));

    let o = s.run(&["/some/dir", "--flag", "x y"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(
        out(&o).trim(),
        format!("fake-ran: /some/dir --flag x y switched={OWN}")
    );

    // `konoma plugin ...` never hands over, so a broken program can be repaired
    let o = s.run(&["plugin", "list"]);
    assert!(out(&o).contains("konoma-foo"), "{}", out(&o));

    // `konoma ./plugin` is a folder, not the command: it is handed over like any other argument
    let o = s.run(&["./plugin"]);
    assert!(out(&o).contains("fake-ran: ./plugin"), "{}", out(&o));
}

#[test]
fn the_loop_guard_stops_a_second_hand_over() {
    let s = Sandbox::new("plugin-cli-guard");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    let o = s.run_env(&["--version"], &[("KONOMA_PLUGIN_SWITCHED_FROM", "0.0.1")]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(
        out(&o).trim(),
        format!("konoma {OWN}"),
        "the standard konoma ran: it did not hand over again"
    );
}

#[test]
fn a_broken_rebuilt_program_does_not_stop_konoma_plugin() {
    let s = Sandbox::new("plugin-cli-brokenbin");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    write_exe(&s.program(), "not a program\n");
    let o = s.run(&["plugin", "list"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(code(&s.run(&["plugin", "update"])), 0);
    assert!(
        std::fs::read_to_string(s.program())
            .unwrap()
            .starts_with("#!/bin/sh"),
        "repaired by a rebuild"
    );
}

#[test]
fn no_rebuilt_program_means_no_hand_over() {
    let s = Sandbox::new("plugin-cli-nohand");
    let o = s.run(&["--version"]);
    assert_eq!(out(&o).trim(), format!("konoma {OWN}"));
    // a program without its record is not trusted either
    std::fs::create_dir_all(s.data().join("bin")).unwrap();
    write_exe(&s.program(), FAKE_KONOMA);
    let o = s.run(&["--version"]);
    assert_eq!(out(&o).trim(), format!("konoma {OWN}"));
}

#[test]
fn keep_build_cache_keeps_the_cache_and_clean_removes_it() {
    let s = Sandbox::new("plugin-cli-keep");
    std::fs::create_dir_all(s.config().join("konoma")).unwrap();
    std::fs::write(
        s.config().join("konoma").join("config.toml"),
        "[plugins]\nkeep_build_cache = true\n",
    )
    .unwrap();
    let o = s.run(&["plugin", "install", "konoma-foo", "-y"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(
        s.data()
            .join("build")
            .join("target")
            .join("release")
            .join("konoma")
            .is_file(),
        "the cache stays: {:?}",
        s.leftovers()
    );
    let o = s.run(&["plugin", "clean"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(!s.data().join("build").exists());
    assert!(s.program().is_file(), "clean does not touch the program");
    assert!(s.lock().is_file());
    let o = s.run(&["plugin", "clean"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("No build cache"), "{}", out(&o));
}

#[test]
fn without_keep_build_cache_the_cache_is_gone_even_after_a_failure() {
    let s = Sandbox::new("plugin-cli-nokeep");
    for fail in [None, Some("1")] {
        let envs: Vec<(&str, &str)> = fail.map(|f| ("FAKE_FAIL_BUILD", f)).into_iter().collect();
        let _ = s.run_env(&["plugin", "install", "konoma-foo", "-y"], &envs);
        assert!(s.leftovers().is_empty(), "{fail:?}: {:?}", s.leftovers());
    }
}

#[test]
fn ctrl_c_stops_cargo_and_deletes_the_build_cache() {
    let s = Sandbox::new("plugin-cli-sigint");
    let mark = s.root.join("cargo-started");
    let mut c = s.command(&["plugin", "install", "konoma-foo", "-y"]);
    c.env("FAKE_MARK", &mark)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !mark.exists() {
        assert!(Instant::now() < deadline, "cargo never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !s.leftovers().is_empty(),
        "the work dir exists while cargo runs"
    );
    let st = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .supervised_status()
        .unwrap();
    assert!(st.success());
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 130, "{}", err(&o));
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
    assert!(!s.program().exists() && !s.plugins_toml().exists());
}

// ---------------------------------------------------------------------------------------------
// helpers for the tests below

/// Stops a process by its pid when dropped (a test that fails half way must not leave it).
struct KillOnDrop(u32);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-KILL", &self.0.to_string()])
            .stderr(Stdio::null())
            .supervised_status();
    }
}

fn signal(pid: u32, sig: &str) {
    let st = Command::new("kill")
        .args([sig, &pid.to_string()])
        .supervised_status()
        .unwrap();
    assert!(st.success(), "kill {sig} {pid}");
}

/// A process counts as dead when `ps` no longer lists it or it is a zombie (state `Z`).
/// `kill -0` is not enough: a zombie nobody reaps (e.g. when PID 1 in a container is a bare
/// `sleep`) still succeeds, so a finished process would be waited for until the timeout.
fn alive(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .supervised_output()
        .expect("`ps` is needed to check whether a process is alive");
    let stat = String::from_utf8_lossy(&out.stdout);
    !matches!(stat.trim().chars().next(), None | Some('Z'))
}

/// Wait for `cond`, at most `secs`; panics with `what` otherwise.
fn wait_until(secs: u64, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for a child to end, at most `secs`; a child that does not is killed and the test fails.
fn wait_exit(child: &mut std::process::Child, secs: u64, what: &str) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            return st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timed out: {what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A pseudo-terminal: what `isatty` on the slave says yes to, with nobody typing on the master.
fn pty() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors that are owned by the Files made from them.
    unsafe {
        let r = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(r, 0, "openpty");
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}

#[test]
fn ctrl_c_at_the_question_ends_the_command() {
    use std::os::unix::process::ExitStatusExt;
    let s = Sandbox::new("plugin-cli-ask-sigint");
    let (_master, slave) = pty();
    let said = s.root.join("stdout.txt");
    let mut c = s.command(&["plugin", "install", "konoma-foo"]);
    c.stdin(Stdio::from(slave))
        .stdout(File::create(&said).unwrap())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let _guard = KillOnDrop(child.id());
    wait_until(30, "the question", || {
        std::fs::read_to_string(&said).is_ok_and(|t| t.contains("[y/N]"))
    });
    signal(child.id(), "-INT");
    // The default action of SIGINT: the process is ended by it, as it is by any command waiting
    // for an answer.
    let st = wait_exit(&mut child, 10, "Ctrl-C at the question");
    assert_eq!(st.signal(), Some(libc::SIGINT), "{st:?}");
    assert!(
        !s.cargo_log().contains("build --release"),
        "nothing was built"
    );
    assert!(!s.plugins_toml().exists() && !s.program().exists());
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

// A replacement that stops half way says what is new and what is old, and one command repairs it.

#[test]
fn a_record_that_cannot_be_written_leaves_the_list_ahead_and_says_so() {
    let s = Sandbox::new("plugin-cli-partial-record");
    // A directory where the record goes: renaming a file over it fails.
    std::fs::create_dir_all(s.record()).unwrap();
    let o = s.run(&["plugin", "install", "konoma-foo", "-y"]);
    assert_eq!(code(&o), 1);
    let e = err(&o);
    assert!(e.contains("stopped half way"), "{e}");
    assert!(
        e.contains("Now new:  Cargo.lock, plugins.toml"),
        "what is new: {e}"
    );
    assert!(
        e.contains("Still old: the record of the program, the konoma program"),
        "what is old: {e}"
    );
    assert!(e.contains("konoma plugin update"), "the repair: {e}");
    assert!(!s.program().exists(), "the program is the last to change");
    assert!(
        std::fs::read_to_string(s.plugins_toml())
            .unwrap()
            .contains("konoma-foo"),
        "what was asked for is saved"
    );
    assert!(s.lock().is_file());
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());

    // The repair named in the message works.
    std::fs::remove_dir(s.record()).unwrap();
    let o = s.run(&["plugin", "update"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(s.program().is_file() && s.record().is_file());
}

#[test]
fn a_program_that_cannot_be_put_in_place_is_the_last_to_fail() {
    let s = Sandbox::new("plugin-cli-partial-program");
    std::fs::create_dir_all(s.program()).unwrap();
    let o = s.run(&["plugin", "install", "konoma-foo", "-y"]);
    assert_eq!(code(&o), 1);
    let e = err(&o);
    assert!(
        e.contains("Now new:  Cargo.lock, plugins.toml, the record of the program"),
        "{e}"
    );
    assert!(
        e.contains("Still old: the konoma program (the previous one keeps running)"),
        "{e}"
    );
    assert!(s.leftovers().is_empty(), "the staged copy is deleted");
}

#[test]
fn a_lock_that_cannot_be_written_changes_nothing() {
    let s = Sandbox::new("plugin-cli-partial-lock");
    std::fs::create_dir_all(s.lock()).unwrap();
    let o = s.run(&["plugin", "install", "konoma-foo", "-y"]);
    assert_eq!(code(&o), 1);
    let e = err(&o);
    assert!(
        s.cargo_log().is_empty(),
        "cannot restore saved lock: cargo must not run"
    );
    assert!(e.contains("Nothing was replaced"), "{e}");
    assert!(!e.contains("Now new"), "{e}");
    assert!(!s.plugins_toml().exists() && !s.record().exists() && !s.program().exists());
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

#[test]
fn removing_the_last_plugin_with_an_unwritable_list_says_how_to_finish() {
    let s = Sandbox::new("plugin-cli-partial-remove");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    // Writing a file fails whoever runs this (root included): the size limit for files is 0, and
    // SIGXFSZ is ignored so that the write returns an error instead of ending the process. The
    // program and the record are only deleted, which the limit does not touch. A `[konoma]` table
    // keeps the file from being empty after the removal: an empty write cannot exceed any limit.
    let text = std::fs::read_to_string(s.plugins_toml()).unwrap();
    std::fs::write(
        s.plugins_toml(),
        format!("[konoma]\npath = \"/some/source\"\n\n{text}"),
    )
    .unwrap();
    let mut c = s.command(&["plugin", "remove", "konoma-foo"]);
    // SAFETY: the closure calls only setrlimit and signal, both async-signal-safe.
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &none) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            Ok(())
        });
    }
    let o = c.supervised_output().expect("start konoma");
    assert_eq!(code(&o), 1);
    let e = err(&o);
    assert!(
        e.contains("standard build again") && e.contains("still lists `konoma-foo`"),
        "{e}"
    );
    assert!(e.contains("konoma plugin remove konoma-foo"), "{e}");
    assert!(!s.program().exists(), "the standard konoma runs");
    // Running the same command again finishes it.
    let o = s.run(&["plugin", "remove", "konoma-foo"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    assert!(!std::fs::read_to_string(s.plugins_toml())
        .unwrap()
        .contains("konoma-foo"));
}

// Two `konoma plugin` at once: the second waits.

#[test]
fn a_second_plugin_command_waits_for_the_lock_and_list_does_not() {
    let s = Sandbox::new("plugin-cli-lock");
    std::fs::create_dir_all(s.data()).unwrap();
    let held = File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(s.data().join("plugin.lock"))
        .unwrap();
    held.lock().unwrap();

    // Reading needs no lock.
    let o = s.run(&["plugin", "list"]);
    assert_eq!(code(&o), 0, "{}", err(&o));

    let said = s.root.join("stderr.txt");
    let mut c = s.command(&["plugin", "install", "konoma-foo", "-y"]);
    c.stdout(Stdio::null()).stderr(File::create(&said).unwrap());
    let mut child = c.spawn().unwrap();
    let _guard = KillOnDrop(child.id());
    wait_until(30, "the notice that it waits", || {
        std::fs::read_to_string(&said).is_ok_and(|t| t.contains("waiting"))
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "still waiting");
    assert!(
        s.cargo_log().lines().all(|l| l.starts_with("metadata ")) && !s.program().exists(),
        "only metadata may precede the lock"
    );

    drop(held);
    let st = wait_exit(&mut child, 30, "the install after the lock was freed");
    assert!(st.success(), "{st:?}");
    assert!(s.program().is_file());
}

#[test]
fn every_command_that_changes_things_takes_the_lock() {
    let s = Sandbox::new("plugin-cli-lock-all");
    assert_eq!(code(&s.run(&["plugin", "install", "konoma-foo", "-y"])), 0);
    let held = File::options()
        .write(true)
        .open(s.data().join("plugin.lock"))
        .unwrap();
    held.lock().unwrap();
    for args in [
        &["plugin", "update"][..],
        &["plugin", "remove", "konoma-foo"][..],
        &["plugin", "clean"][..],
    ] {
        let said = s.root.join("stderr-all.txt");
        let mut c = s.command(args);
        c.stdout(Stdio::null()).stderr(File::create(&said).unwrap());
        let mut child = c.spawn().unwrap();
        let _guard = KillOnDrop(child.id());
        wait_until(30, &format!("{args:?} to wait"), || {
            std::fs::read_to_string(&said).is_ok_and(|t| t.contains("waiting"))
        });
        assert!(child.try_wait().unwrap().is_none(), "{args:?} waits");
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(held);
}

// SIGTERM and Ctrl-C stop everything cargo started, not only cargo.

fn stops_the_whole_group(name: &str, sig: &str, ignore_term: bool, want_code: i32) {
    let s = Sandbox::new(name);
    let mark = s.root.join("cargo-started");
    let pidfile = s.root.join("grandchild.pid");
    let mut c = s.command(&["plugin", "install", "konoma-foo", "-y"]);
    c.env("FAKE_MARK", &mark)
        .env("FAKE_GRANDCHILD", &pidfile)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if ignore_term {
        c.env("FAKE_GC_IGNORE_TERM", "1");
    }
    let mut child = c.spawn().unwrap();
    let _guard = KillOnDrop(child.id());
    wait_until(30, "cargo to start its process", || mark.exists());
    let gc: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _gc_guard = KillOnDrop(gc);
    assert!(alive(gc), "the grandchild runs while cargo waits");
    signal(child.id(), sig);
    let st = wait_exit(&mut child, 20, "konoma to stop");
    assert_eq!(st.code(), Some(want_code), "{st:?}");
    wait_until(10, "the grandchild to end", || !alive(gc));
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
    assert!(!s.program().exists() && !s.plugins_toml().exists());
}

#[test]
fn sigterm_stops_the_processes_cargo_started_too() {
    stops_the_whole_group("plugin-cli-term-group", "-TERM", false, 130);
}

#[test]
fn ctrl_c_stops_a_process_that_ignores_sigterm_too() {
    stops_the_whole_group("plugin-cli-int-group", "-INT", true, 130);
}

#[test]
fn closing_the_terminal_stops_the_processes_cargo_started_too() {
    // SIGHUP does not reach cargo (its own process group), so konoma must pass it on.
    stops_the_whole_group("plugin-cli-hup-group", "-HUP", false, 130);
}

#[test]
fn an_interrupt_while_the_new_build_starts_keeps_the_previous_konoma() {
    let s = Sandbox::new("plugin-cli-int-verify");
    let mark = s.root.join("verify-started");
    let mut c = s.command(&["plugin", "install", "konoma-foo", "-y"]);
    c.env("FAKE_VERIFY_MARK", &mark)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let _guard = KillOnDrop(child.id());
    wait_until(30, "the new build to be asked for --plugins", || {
        mark.exists()
    });
    let verifier: u32 = std::fs::read_to_string(&mark)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _v_guard = KillOnDrop(verifier);
    assert!(alive(verifier));
    signal(child.id(), "-INT");
    let o = {
        let st = wait_exit(&mut child, 20, "konoma to stop");
        let mut text = String::new();
        use std::io::Read;
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        (st, text)
    };
    assert_eq!(o.0.code(), Some(130), "{o:?}");
    assert!(
        o.1.contains("interrupted; the previous konoma is unchanged"),
        "{}",
        o.1
    );
    assert!(!o.1.contains("did not finish"), "{}", o.1);
    wait_until(10, "the new build to end", || !alive(verifier));
    assert!(!s.program().exists() && !s.record().exists() && !s.plugins_toml().exists());
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

/// Linux only (it needs PR_SET_PDEATHSIG): when konoma is killed with SIGKILL, cargo is told to
/// stop. The fake cargo passes the SIGTERM on to its group, as a build tool does for its jobs.
/// Not run on macOS, where nothing like this exists.
#[cfg(target_os = "linux")]
#[test]
fn killing_konoma_with_sigkill_still_ends_the_build() {
    let s = Sandbox::new("plugin-cli-pdeathsig");
    let mark = s.root.join("cargo-started");
    let pidfile = s.root.join("grandchild.pid");
    let mut c = s.command(&["plugin", "install", "konoma-foo", "-y"]);
    c.env("FAKE_MARK", &mark)
        .env("FAKE_GRANDCHILD", &pidfile)
        .env("FAKE_TERM_KILLS_GROUP", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    let _guard = KillOnDrop(child.id());
    wait_until(30, "cargo to start its process", || mark.exists());
    let gc: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _gc_guard = KillOnDrop(gc);
    assert!(alive(gc));
    signal(child.id(), "-KILL");
    let _ = wait_exit(&mut child, 20, "konoma to die");
    wait_until(10, "the grandchild to end", || !alive(gc));
}

// ---------------------------------------------------------------------------------------------
// The variable of the hand-over is not passed on.

#[test]
fn the_hand_over_variable_does_not_reach_what_konoma_starts() {
    let s = Sandbox::new("plugin-cli-envleak");
    // A plugin program of its own is a child of konoma (`--plugin`); it writes what it sees.
    let seen = s.root.join("seen.txt");
    let prog = s.root.join("probe-plugin");
    write_exe(
        &prog,
        &format!(
            "#!/bin/sh\necho \"[${{KONOMA_PLUGIN_SWITCHED_FROM-unset}}]\" >> {}\nexit 1\n",
            seen.display()
        ),
    );
    let o = s.run_env(
        &["--plugins", "--plugin", &prog.to_string_lossy()],
        &[("KONOMA_PLUGIN_SWITCHED_FROM", "0.0.1")],
    );
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    let text = std::fs::read_to_string(&seen).expect("the plugin program was started");
    assert!(text.contains("[unset]"), "{text}");
    assert!(!text.contains("0.0.1"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// The hand-written plugins.toml keeps its comments and its order.

#[test]
fn install_and_remove_keep_the_comments_and_order_of_plugins_toml() {
    let s = Sandbox::new("plugin-cli-keep-comments");
    let hand = "# my plugins\n\n# the office tools\n[plugins.konoma-zeta]\nversion = \"1\"   # pinned\n\n[plugins.konoma-alpha]\nversion = \"2\"\n";
    std::fs::create_dir_all(s.plugins_toml().parent().unwrap()).unwrap();
    std::fs::write(s.plugins_toml(), hand).unwrap();

    let o = s.run(&["plugin", "install", "konoma-foo", "-y"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    let after = std::fs::read_to_string(s.plugins_toml()).unwrap();
    assert!(
        after.starts_with(hand),
        "the old text is untouched:\n{after}"
    );
    assert!(after.contains("[plugins.konoma-foo]"), "{after}");

    let o = s.run(&["plugin", "remove", "konoma-alpha"]);
    assert_eq!(code(&o), 0, "{} {}", out(&o), err(&o));
    let after = std::fs::read_to_string(s.plugins_toml()).unwrap();
    assert!(
        after.contains("# my plugins") && after.contains("# the office tools"),
        "{after}"
    );
    assert!(after.contains("version = \"1\"   # pinned"), "{after}");
    assert!(!after.contains("alpha"), "{after}");
    let (z, f) = (
        after.find("konoma-zeta").unwrap(),
        after.find("konoma-foo").unwrap(),
    );
    assert!(z < f, "the order is the order of the file:\n{after}");
}

// ---------------------------------------------------------------------------------------------
// What is handed over to the rebuilt konoma, and what is not.

/// A fake rebuilt program (it prints its arguments) with its record.
fn fake_rebuilt(s: &Sandbox) {
    std::fs::create_dir_all(s.data().join("bin")).unwrap();
    write_exe(&s.program(), FAKE_KONOMA);
    std::fs::write(
        s.record(),
        format!("konoma_version = \"{OWN}\"\n[plugins]\nkonoma-foo = \"0.3.2\"\n"),
    )
    .unwrap();
}

#[test]
fn the_drawing_child_is_never_handed_over() {
    let s = Sandbox::new("plugin-cli-svgchild");
    fake_rebuilt(&s);
    let o = s.run(&["--internal-svg-render", "1000000000", "1000"]);
    let all = format!("{}{}", out(&o), err(&o));
    assert!(
        !all.contains("fake-ran") && !all.contains("fake-konoma"),
        "the real binary drew: {all}"
    );
}

#[test]
fn a_dev_plugin_flag_is_handed_over_with_its_arguments() {
    let s = Sandbox::new("plugin-cli-devplugin");
    fake_rebuilt(&s);
    let o = s.run(&["--plugin", "./p", "/some/dir"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(
        out(&o).trim(),
        format!("fake-ran: --plugin ./p /some/dir switched={OWN}"),
        "the rebuilt program gets the flag, so it loads the dev plugin"
    );
}

#[test]
fn version_and_help_are_answered_by_the_rebuilt_program() {
    let s = Sandbox::new("plugin-cli-versionhelp");
    fake_rebuilt(&s);
    // Both are handed over like any other start: the rebuilt konoma tells its own version (with
    // its plugins) and its own help.
    let o = s.run(&["--version"]);
    assert_eq!(out(&o).trim(), format!("fake-konoma from={OWN}"));
    let o = s.run(&["-V"]);
    assert_eq!(out(&o).trim(), format!("fake-ran: -V switched={OWN}"));
    for flag in ["--help", "-h"] {
        let o = s.run(&[flag]);
        assert_eq!(
            out(&o).trim(),
            format!("fake-ran: {flag} switched={OWN}"),
            "{flag}"
        );
    }
}

/// The real cargo: a plugin crate by `path` (a minimal one, written here) on top of this very
/// source (`[konoma] path`). Slow and needs the cargo registry cache (offline); run with
/// `KONOMA_REAL_CARGO_DIR=<a directory under .cache> cargo nextest run -p konoma --run-ignored only -E 'test(real_cargo)'`.
#[test]
#[ignore = "builds konoma with the real cargo (minutes); needs KONOMA_REAL_CARGO_DIR"]
fn real_cargo_builds_konoma_with_a_path_plugin() {
    let dir = PathBuf::from(
        std::env::var_os("KONOMA_REAL_CARGO_DIR")
            .expect("set KONOMA_REAL_CARGO_DIR to a directory under .cache"),
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let real_home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home.join(".cargo"));
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home.join(".rustup"));

    let plug = dir.join("konoma-testplug");
    std::fs::create_dir_all(plug.join("src")).unwrap();
    std::fs::write(
        plug.join("Cargo.toml"),
        format!(
            "[package]\nname = \"konoma-testplug\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\n\n[dependencies]\nkonoma-plugin = {{ path = \"{}\" }}\n",
            root.join("crates/konoma-plugin").display()
        ),
    )
    .unwrap();
    std::fs::write(
        plug.join("src/lib.rs"),
        r#"use konoma_plugin::prelude::*;
struct TestPlug;
impl Plugin for TestPlug {
    fn manifest(&self) -> Manifest {
        assert!(std::env::var_os("KONOMA_PLUGIN_SWITCHED_FROM").is_none());
        assert!(std::thread::spawn(|| std::process::Command::new("/bin/sh")
            .args(["-c", "test -z \"${KONOMA_PLUGIN_SWITCHED_FROM+x}\""])
            .status().unwrap().success()).join().unwrap());
        Manifest::new("pr4-testplug", "0.1.0").bind(Bind::files(["*.pr4-testplug"])).shows(ViewKind::Text)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported(t!(en: "fixture", ja: "試験")))
    }
}
pub fn plugins() -> Vec<Box<dyn Plugin>> {
    assert!(std::env::var_os("KONOMA_PLUGIN_SWITCHED_FROM").is_none());
    assert!(std::env::var_os("KONOMA_PLUGIN_SWITCHED_PATH").is_none());
    vec![Box::new(TestPlug)]
}
"#,
    )
    .unwrap();

    let home = dir.join("home");
    let cfg = dir.join("config");
    let data = dir.join("data");
    let compiler_tmp = dir.join("compiler-tmp");
    for d in [&home, &cfg.join("konoma"), &data, &compiler_tmp] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        cfg.join("konoma").join("plugins.toml"),
        format!("[konoma]\npath = \"{}\"\n", root.display()),
    )
    .unwrap();
    // Offline, a fresh resolution would trip over yanked versions: start from this tree's lock
    // (`install` keeps the saved lock, as it does for the user).
    std::fs::create_dir_all(data.join("konoma")).unwrap();
    std::fs::copy(root.join("Cargo.lock"), data.join("konoma/Cargo.lock")).unwrap();
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_konoma"))
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &cfg)
            .env("XDG_DATA_HOME", &data)
            .env("CARGO_HOME", &cargo_home)
            .env("RUSTUP_HOME", &rustup_home)
            .env("CARGO_NET_OFFLINE", "true")
            .env("TMPDIR", &compiler_tmp)
            .current_dir(&home)
            .stdin(Stdio::null())
            .supervised_output()
            .unwrap()
    };
    let t = Instant::now();
    let o = run(&[
        "plugin",
        "install",
        "konoma-testplug",
        "--path",
        &plug.to_string_lossy(),
        "--yes",
    ]);
    eprintln!("install took {:?}", t.elapsed());
    assert_eq!(code(&o), 0, "{}\n{}", out(&o), err(&o));
    let bin = data.join("konoma/bin/konoma");
    let size = std::fs::metadata(&bin).unwrap().len();
    eprintln!("rebuilt program: {size} bytes");
    let o = run(&["--version"]);
    assert!(
        out(&o).contains("konoma-testplug"),
        "{}\n{}",
        out(&o),
        err(&o)
    );
    let o = run(&["--plugins"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("markdown"), "{}", out(&o));
    assert!(
        out(&o)
            .lines()
            .any(|line| line.split_whitespace().any(|word| word == "pr4-testplug")),
        "the added plugin must be registered: {}",
        out(&o)
    );
    let o = run(&["plugin", "list"]);
    assert!(out(&o).contains("konoma-testplug"), "{}", out(&o));
    let o = run(&["plugin", "remove", "konoma-testplug"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(!bin.exists());
    let left: Vec<_> = std::fs::read_dir(data.join("konoma"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    eprintln!("data dir after remove: {left:?}");
}
