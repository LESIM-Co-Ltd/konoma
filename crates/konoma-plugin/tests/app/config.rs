//! The settings a plugin receives in the standalone app: `[plugins.<id>]` and the old `[ui]` names
//! read the way konoma reads them, and the config file found through `XDG_CONFIG_HOME`.

use std::sync::{Arc, Mutex};

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::{keys_from_load, open_file_with, AppError};
use konoma_plugin::config::plugin_settings;
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::{unique_tmp, TmpDir};

/// A plugin that keeps the settings it was opened with.
struct Spy {
    id: &'static str,
    seen: Arc<Mutex<Option<Settings>>>,
}

impl Plugin for Spy {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id, "0.0.1")
            .bind(Bind::files(["*.spy"]))
            .shows(ViewKind::Text)
    }

    fn open(&self, _input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        *self.seen.lock().unwrap() = Some(cx.settings.clone());
        Err(Failure::unsupported(t!(en: "spy: seen")))
    }
}

fn file() -> (TmpDir, std::path::PathBuf) {
    let d = unique_tmp("app_config_spy");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let p = d.as_path().join("a.spy");
    std::fs::write(&p, "x").unwrap();
    (d, p)
}

/// What plugin `id` is opened with when `config` is the config file and the file is opened the way
/// `app::run` does it.
fn settings_seen(id: &'static str, config: &str) -> Settings {
    let (_d, p) = file();
    let cfg = SharedConfig::parse(config).expect("the test config parses");
    let seen = Arc::new(Mutex::new(None));
    let plugin: Arc<dyn Plugin> = Arc::new(Spy {
        id,
        seen: Arc::clone(&seen),
    });
    let of = |id: &str| plugin_settings(&cfg.plugins, &cfg.ui.moved(), id);
    // The spy refuses to open (it only wants to look), which is fine: the settings are kept first.
    let _ = open_file_with(vec![plugin], &p, "en", TermCaps::new(), &of);
    let got = seen.lock().unwrap().take();
    got.expect("the plugin was opened")
}

#[test]
fn the_plugin_table_reaches_the_plugin_without_the_hosts_keys() {
    let s = settings_seen(
        "markdown",
        "[plugins.markdown]\nemoji = false\ndetails = \"open\"\nenabled = true\nread_only = true\n",
    );
    assert_eq!(s.get("emoji"), Some(&Value::Bool(false)));
    assert_eq!(s.str("details", ""), "open");
    assert!(s.get("enabled").is_none() && s.get("read_only").is_none());
}

#[test]
fn an_old_ui_name_reaches_its_plugin_and_the_new_name_wins() {
    let s = settings_seen(
        "markdown",
        "[ui]\nmd_alerts = false\nmd_emoji = false\n[plugins.markdown]\nemoji = true\n",
    );
    assert_eq!(s.get("alerts"), Some(&Value::Bool(false)), "old name");
    assert_eq!(s.get("emoji"), Some(&Value::Bool(true)), "new name wins");
}

#[test]
fn another_plugin_does_not_get_them() {
    let s = settings_seen(
        "other",
        "[ui]\nmd_alerts = false\n[plugins.markdown]\nemoji = true\n",
    );
    assert!(s.get("alerts").is_none() && s.get("emoji").is_none());
}

#[test]
fn a_value_of_the_wrong_type_is_passed_on_and_the_plugin_reads_its_default() {
    // The same rule as konoma: the table is passed as written and the plugin's typed read falls
    // back (konoma names the mistake at startup; the small host has no startup message).
    let s = settings_seen("markdown", "[plugins.markdown]\nemoji = \"no\"\n");
    assert_eq!(s.get("emoji"), Some(&Value::Str("no".into())));
    assert!(s.bool("emoji", true));
}

#[test]
fn without_a_config_the_plugin_gets_the_values_konoma_would_hand_it() {
    // Konoma completes a plugin's settings with the defaults of the moved `[ui]` keys; the app
    // does the same (konoma's own side is pinned equal in its settings parity tests).
    let s = settings_seen("markdown", "");
    let want = plugin_settings(
        &std::collections::BTreeMap::new(),
        &konoma_host_core::shared::MovedUi::default(),
        "markdown",
    );
    assert_eq!(s, want);
    assert!(
        s.get("alerts").is_some(),
        "the defaults are really handed over"
    );
}

#[test]
fn open_file_alone_passes_no_settings() {
    let (_d, p) = file();
    let seen = Arc::new(Mutex::new(None));
    let plugin: Arc<dyn Plugin> = Arc::new(Spy {
        id: "markdown",
        seen: Arc::clone(&seen),
    });
    let _ = konoma_plugin::app::open_file(vec![plugin], &p, "en", TermCaps::new());
    let got = seen.lock().unwrap().take();
    assert_eq!(got.expect("opened"), Settings::new());
}

#[test]
fn a_broken_config_gives_the_defaults_and_one_reported_line() {
    let d = unique_tmp("app_config_broken");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let path = d.as_path().join("config.toml");
    std::fs::write(&path, "[plugins.markdown\nemoji = ").unwrap();
    let (cfg, err) = SharedConfig::load(&path);
    assert!(cfg.plugins.is_empty());
    let err = err.expect("reported");
    assert!(err.starts_with("config error (using defaults): "), "{err}");
    let (_keys, passed) = keys_from_load((cfg, Some(err.clone())));
    assert_eq!(passed.as_deref(), Some(err.as_str()));
}

// The config file's place. `app::run` loads `SharedConfig::load_default`, which follows
// `XDG_CONFIG_HOME` (when set and not empty) and `HOME` otherwise. Environment variables are
// process-wide, so each case runs in a child process of this test binary with its own
// environment, in the way of the XDG test of konoma's own config; the user's real files are never
// read (both variables point into a temporary directory).

const CHILD_MARK: &str = "KONOMA_APP_XDG_CHILD";

fn write_config(dir: &std::path::Path, lang: &str, alerts: bool) {
    let d = dir.join("konoma");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("config.toml"),
        format!("[ui]\nlang = \"{lang}\"\n[plugins.markdown]\nalerts = {alerts}\n"),
    )
    .unwrap();
}

/// Run the child half of `test` with `xdg` as `XDG_CONFIG_HOME` (unset when `None`) and `home`
/// as `HOME`; `want` is handed to the child in `CHILD_MARK`. The child's standard input and output
/// are not a terminal. Returns the finished child (it has been checked to have run the test).
fn spawn_child(
    test: &str,
    xdg: Option<&std::path::Path>,
    home: &std::path::Path,
    want: &str,
) -> std::process::Output {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args([
        &format!("config::{test}"),
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ])
    .env(CHILD_MARK, want)
    .env("HOME", home)
    .stdin(std::process::Stdio::null());
    match xdg {
        Some(x) => cmd.env("XDG_CONFIG_HOME", x),
        None => cmd.env_remove("XDG_CONFIG_HOME"),
    };
    let out = cmd.output().expect("the test binary starts");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The child really ran the test (a filter that matches nothing also exits 0).
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 passed"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    out
}

fn in_child(test: &str, xdg: Option<&std::path::Path>, home: &std::path::Path, want: &str) {
    spawn_child(test, xdg, home, want);
}

/// The child half: what the loader finds under this environment.
fn child_loads(test_name: &str) -> bool {
    let Ok(want) = std::env::var(CHILD_MARK) else {
        return false;
    };
    let (cfg, err) = SharedConfig::load_default();
    assert!(err.is_none(), "{test_name}: {err:?}");
    let alerts = plugin_settings(&cfg.plugins, &cfg.ui.moved(), "markdown").bool("alerts", true);
    assert_eq!(format!("{}/{alerts}", cfg.ui.lang), want, "{test_name}");
    true
}

#[test]
fn the_config_is_read_from_xdg_config_home_and_not_from_home() {
    if child_loads("xdg") {
        return;
    }
    let d = unique_tmp("app_xdg");
    let (xdg, home) = (d.as_path().join("xdg"), d.as_path().join("home"));
    write_config(&xdg, "jp", false);
    write_config(&home.join(".config"), "en", true);
    in_child(
        "the_config_is_read_from_xdg_config_home_and_not_from_home",
        Some(&xdg),
        &home,
        "jp/false",
    );
}

#[test]
fn an_empty_xdg_config_home_means_home_config() {
    if child_loads("empty xdg") {
        return;
    }
    let d = unique_tmp("app_xdg_empty");
    let home = d.as_path().join("home");
    write_config(&home.join(".config"), "jp", false);
    in_child(
        "an_empty_xdg_config_home_means_home_config",
        Some(std::path::Path::new("")),
        &home,
        "jp/false",
    );
}

#[test]
fn without_xdg_config_home_it_is_home_config() {
    if child_loads("no xdg") {
        return;
    }
    let d = unique_tmp("app_xdg_unset");
    let home = d.as_path().join("home");
    write_config(&home.join(".config"), "en", false);
    in_child(
        "without_xdg_config_home_it_is_home_config",
        None,
        &home,
        "en/false",
    );
}

// `app::run` before the terminal: the config found through the environment reaches the plugin
// (`prepare`, which `run` calls with `SharedConfig::load_default()`), and a config error is still
// reported when the run ends before the screen.

const WIRING_CONFIG: &str = "[ui]\nlang = \"en\"\nmd_alerts = false\n\
    [plugins.markdown]\nemoji = false\ndetails = \"open\"\nenabled = true\n";

fn write_raw_config(dir: &std::path::Path, text: &str) {
    let d = dir.join("konoma");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("config.toml"), text).unwrap();
}

#[test]
fn the_config_file_reaches_the_plugin_through_the_wiring_of_run() {
    if std::env::var(CHILD_MARK).is_ok() {
        let (_d, p) = file();
        let seen = Arc::new(Mutex::new(None));
        let plugin: Arc<dyn Plugin> = Arc::new(Spy {
            id: "markdown",
            seen: Arc::clone(&seen),
        });
        let r = konoma_plugin::app::prepare(
            vec![plugin],
            &p,
            TermCaps::new(),
            SharedConfig::load_default(),
        );
        assert!(r.is_err(), "the spy refuses to open");
        let s = seen.lock().unwrap().take().expect("the plugin was opened");
        assert_eq!(s.get("emoji"), Some(&Value::Bool(false)), "[plugins] value");
        assert_eq!(s.str("details", ""), "open", "[plugins] value");
        assert!(s.get("enabled").is_none(), "the host's key stays out");
        assert_eq!(s.get("alerts"), Some(&Value::Bool(false)), "old [ui] name");
        assert!(s.get("footnotes").is_some(), "a default fills in");
        return;
    }
    let d = unique_tmp("app_wiring");
    let (xdg, home) = (d.as_path().join("xdg"), d.as_path().join("home"));
    write_raw_config(&xdg, WIRING_CONFIG);
    in_child(
        "the_config_file_reaches_the_plugin_through_the_wiring_of_run",
        Some(&xdg),
        &home,
        "1",
    );
}

#[test]
fn a_broken_config_is_reported_when_the_plugin_refuses_the_file() {
    if std::env::var(CHILD_MARK).is_ok() {
        let (_d, p) = file();
        let plugin: Arc<dyn Plugin> = Arc::new(Spy {
            id: "markdown",
            seen: Arc::new(Mutex::new(None)),
        });
        let e = konoma_plugin::app::run(vec![plugin], &p).expect_err("the spy refuses");
        println!("RESULT: {e}");
        return;
    }
    let d = unique_tmp("app_broken_refused");
    let (xdg, home) = (d.as_path().join("xdg"), d.as_path().join("home"));
    write_raw_config(&xdg, "[plugins.markdown\nemoji = ");
    let out = spawn_child(
        "a_broken_config_is_reported_when_the_plugin_refuses_the_file",
        Some(&xdg),
        &home,
        "1",
    );
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        so.contains("RESULT: cannot open the file: spy: seen"),
        "{so}"
    );
    assert_eq!(
        se.matches("config error (using defaults): ").count(),
        1,
        "{se}"
    );
}

#[test]
fn a_broken_config_is_reported_when_there_is_no_terminal() {
    if std::env::var(CHILD_MARK).is_ok() {
        let d = unique_tmp("app_broken_noterm_file");
        std::fs::create_dir_all(d.as_path()).unwrap();
        let p = d.as_path().join("note.t");
        std::fs::write(&p, "x").unwrap();
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(crate::plugin::TestPlugin::all())];
        let e = konoma_plugin::app::run(plugins, &p).expect_err("no terminal here");
        println!("RESULT: {e}");
        return;
    }
    let d = unique_tmp("app_broken_noterm");
    let (xdg, home) = (d.as_path().join("xdg"), d.as_path().join("home"));
    write_raw_config(&xdg, "[plugins.markdown\nemoji = ");
    let out = spawn_child(
        "a_broken_config_is_reported_when_there_is_no_terminal",
        Some(&xdg),
        &home,
        "1",
    );
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        so.contains("RESULT: standard input and output must be a terminal"),
        "{so}"
    );
    assert_eq!(
        se.matches("config error (using defaults): ").count(),
        1,
        "{se}"
    );
}

#[test]
fn a_refusal_is_preceded_by_the_config_line_and_a_good_config_adds_none() {
    let mut w = Vec::new();
    let e = konoma_plugin::app::PrepareFailure {
        error: AppError::NoTerminal,
        load_error: Some("config error (using defaults): x".to_string()),
    }
    .report(&mut w);
    assert!(matches!(e, AppError::NoTerminal));
    assert_eq!(
        String::from_utf8(w).unwrap(),
        "config error (using defaults): x\n"
    );
    let mut w = Vec::new();
    konoma_plugin::app::PrepareFailure {
        error: AppError::NoTerminal,
        load_error: None,
    }
    .report(&mut w);
    assert!(w.is_empty());
}
