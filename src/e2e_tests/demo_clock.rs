//! The clock demo (`crates/konoma-demo-clock`) as a plugin program of its own: the test starts the
//! built program as a real child process through `external::install` (the path of
//! `[plugins.<id>] command`), opens `samples/demo.konoma-demo` and drives it with keys. The line editor
//! takes the typing, `Esc` leaves it, and an action on an arrow key changes what is on screen.
//!
//! The program must be built first (`cargo build -p konoma-demo-clock`); a missing one fails the
//! tests with that instruction, it is never skipped.

use super::*;
use crate::i18n::Lang;
use crate::plugins::external::{self, Env};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

/// `target/<profile>/konoma-demo-clock`, found from the test executable (`target/<profile>/deps/`).
fn built_program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("konoma-demo-clock");
    assert!(
        p.is_file(),
        "{} is missing: run `cargo build -p konoma-demo-clock` before these tests",
        p.display()
    );
    p
}

/// The processes whose command line names `exe`: the lines of `ps` (what is alive, not what this
/// process thinks it started).
fn processes_of(exe: &Path) -> Vec<String> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,command="])
        .output()
        .unwrap();
    let needle = exe.to_string_lossy().into_owned();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(&needle))
        .map(str::to_string)
        .collect()
}

/// Stops the test's child when the test ends and checks that none is left. No waiting is involved:
/// `shutdown_set` returns after the child was asked to leave and was reaped.
struct Guard {
    token: String,
    exe: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        external::shutdown_set(&self.token);
        if !std::thread::panicking() {
            let left = processes_of(&self.exe);
            assert!(
                left.is_empty(),
                "the plugin program is still alive: {left:?}"
            );
        }
    }
}

/// A browsing session whose plugin is the clock program. Field order matters: the session goes
/// before the guard that stops the child, and both before the directory.
struct Demo {
    s: Sim,
    guard: Guard,
    _dir: crate::test_support::TmpDir,
}

impl Deref for Demo {
    type Target = Sim;
    fn deref(&self) -> &Sim {
        &self.s
    }
}

impl DerefMut for Demo {
    fn deref_mut(&mut self) -> &mut Sim {
        &mut self.s
    }
}

/// Opens `samples/demo.konoma-demo` with the built program loaded the way `--plugin` loads it. The
/// program is copied to a path of this test alone, so that the processes of other tests (nextest
/// runs them side by side) never count as this one's.
fn open_demo_in(name: &str, lang: Lang) -> Demo {
    let dir = sandbox(name);
    for sub in ["root", "cache", "scratch", "work", "bin"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let exe = canon(&dir.join("bin")).join("konoma-demo-clock");
    // Copied by a `cp` child, not `fs::copy`: a write fd held here can leak into a concurrent fork
    // and make running the copy fail with ETXTBSY.
    let status = std::process::Command::new("cp")
        .arg(built_program())
        .arg(&exe)
        .status()
        .unwrap();
    assert!(status.success(), "cp failed: {status}");
    let root = canon(&dir.join("root"));
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/demo.konoma-demo");
    std::fs::copy(src, root.join("demo.konoma-demo")).unwrap();
    // `[plugins.demo-clock] command = "<program>"`, `args = ["--serve"]`: the program serves the host
    // when it is given `--serve`.
    let mut table = toml::Table::new();
    table.insert("command".into(), exe.to_string_lossy().as_ref().into());
    table.insert("args".into(), vec![toml::Value::from("--serve")].into());
    let tables: crate::plugins::PluginTables = [("demo-clock".to_string(), table)].into();
    let env = Env {
        cache_dir: canon(&dir.join("cache")),
        scratch_root: canon(&dir.join("scratch")),
        work_dir: canon(&dir.join("work")),
        path_var: None,
        cwd: root.clone(),
        limits: konoma_plugin::proc::client::Limits::default(),
        handshake_limit: std::time::Duration::from_secs(30),
        host_version: env!("CARGO_PKG_VERSION").to_string(),
        lang,
        launcher: None,
    };
    let token = external::install(&env, &tables, &[]).expect("the clock program registers");
    let problems = external::problems(&token, lang);
    assert!(problems.is_empty(), "{problems:?}");
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = if lang == Lang::Jp { "ja" } else { "en" }.into();
    cfg.plugins = tables;
    cfg.external_plugins = Some(token.clone());
    let guard = Guard { token, exe };
    let mut s = Sim::with_config(&root, cfg);
    s.select("demo.konoma-demo");
    s.enter();
    s.draw();
    Demo {
        s,
        guard,
        _dir: dir,
    }
}

fn open_demo(name: &str) -> Demo {
    open_demo_in(name, Lang::En)
}

#[test]
fn e2e_demo_clock_shows_the_title_the_gauge_and_the_hint() {
    let s = open_demo("demo_clock_open");
    assert!(s.app.is_canvas_preview(), "{}", s.screen());
    s.see("konoma canvas demo");
    s.see("0 s");
    s.see("i: type a note");
}

#[test]
fn e2e_demo_clock_typing_enter_esc_and_clear() {
    let mut s = open_demo("demo_clock_type");
    assert!(!s.app.is_plugin_input());
    s.key('i');
    assert!(s.app.is_plugin_input(), "`i` starts the line editor");
    s.keys("hello");
    s.see("> hello█");
    s.enter();
    s.see("- hello");
    s.keys("x");
    s.esc();
    assert!(!s.app.is_plugin_input(), "Esc leaves the capture");
    assert!(s.app.is_canvas_preview(), "{}", s.screen());
    // Back with the host's keys: `?` is the help again, and `c` clears the notes.
    s.key('c');
    assert!(!s.screen().contains("- hello"), "`c` cleared the notes");
}

#[test]
fn e2e_demo_clock_an_arrow_key_changes_the_gauge_colour() {
    let mut s = open_demo("demo_clock_hue");
    s.see("green");
    s.press(KeyCode::Right, KeyModifiers::NONE);
    s.see("magenta");
    s.press(KeyCode::Right, KeyModifiers::NONE);
    s.see("yellow");
    s.press(KeyCode::Left, KeyModifiers::NONE);
    s.see("magenta");
    assert!(s.app.is_canvas_preview(), "the arrow stays in the plugin");
}

#[test]
fn e2e_demo_clock_runs_as_a_child_process_and_leaves_with_the_test() {
    let d = open_demo("demo_clock_child");
    d.see("konoma canvas demo");
    let alive = processes_of(&d.guard.exe);
    assert_eq!(alive.len(), 1, "one child serves the plugin: {alive:?}");
    assert!(alive[0].contains("--serve"), "{alive:?}");
    let reg = crate::plugins::shared_registry(&d.app.cfg.plugin_disabled());
    assert_eq!(
        reg.origin("demo-clock"),
        Some(konoma_plugin::registry::PluginOrigin::User)
    );
}

/// The test terminal draws wide glyphs with a blank after each: compare without blanks.
fn squeezed(s: &Sim) -> String {
    s.screen().chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn e2e_demo_clock_explains_itself_in_english() {
    let mut s = open_demo("demo_clock_help_en");
    s.see("i: type a note");
    s.see("gauge colour");
    assert!(
        !s.screen().contains("c: clear notes"),
        "nothing to clear yet"
    );
    s.key('i');
    s.see("Typing");
    s.see("Esc leaves");
    s.keys("hi");
    s.enter();
    s.see("Notes");
    s.see("- hi");
    s.esc();
    s.see("c: clear notes");
    s.key('c');
    assert!(
        !s.screen().contains("Notes"),
        "the heading goes with the notes"
    );
}

#[test]
fn e2e_demo_clock_explains_itself_in_japanese() {
    let mut s = open_demo_in("demo_clock_help_ja", Lang::Jp);
    assert!(squeezed(&s).contains("i:メモを入力"), "{}", s.screen());
    assert!(squeezed(&s).contains("ゲージの色"));
    assert!(!squeezed(&s).contains("メモを消す"));
    s.key('i');
    assert!(squeezed(&s).contains("入力中"), "{}", s.screen());
    assert!(squeezed(&s).contains("Escで抜ける"));
    s.keys("hello");
    s.see("> hello█");
    s.enter();
    assert!(squeezed(&s).contains("-hello"));
    s.esc();
    assert!(squeezed(&s).contains("c:メモを消す"), "{}", s.screen());
}
