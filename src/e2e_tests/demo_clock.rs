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

/// How the clock program is registered.
enum Via {
    /// `--plugin <program>`: `install(env, &Default::default(), &[program])`.
    Dev,
    /// `[plugins.demo-clock] command = "/bin/sh"`, `args = [<script>, "one", "two words"]`: the program
    /// is reached through an interpreter and a script of the test that records what it was started
    /// with (argv, working directory, `PATH`) and then `exec`s the program with the same arguments.
    /// The record is `<dir>/record`.
    Config,
}

/// The directories of one test, with the program copied to a path of this test alone (so that the
/// processes of other tests, which nextest runs side by side, never count as this one's).
struct Dirs {
    dir: crate::test_support::TmpDir,
    exe: PathBuf,
    root: PathBuf,
}

fn dirs_of(name: &str) -> Dirs {
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
    Dirs { dir, exe, root }
}

fn env_of(d: &Dirs, lang: Lang) -> Env {
    Env {
        cache_dir: canon(&d.dir.join("cache")),
        scratch_root: canon(&d.dir.join("scratch")),
        work_dir: canon(&d.dir.join("work")),
        path_var: None,
        cwd: d.root.clone(),
        limits: konoma_plugin::proc::client::Limits::default(),
        handshake_limit: std::time::Duration::from_secs(30),
        host_version: env!("CARGO_PKG_VERSION").to_string(),
        lang,
        launcher: None,
    }
}

/// A session over the dirs' root with the installed set `token`.
fn session_of(d: &Dirs, token: &str, tables: crate::plugins::PluginTables, lang: Lang) -> Sim {
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = if lang == Lang::Jp { "ja" } else { "en" }.into();
    cfg.plugins = tables;
    cfg.external_plugins = Some(token.to_string());
    Sim::with_config(&d.root, cfg)
}

fn open_demo_via(name: &str, lang: Lang, via: Via) -> Demo {
    let d = dirs_of(name);
    let env = env_of(&d, lang);
    let (tables, devs) = match via {
        Via::Dev => (crate::plugins::PluginTables::new(), vec![d.exe.clone()]),
        Via::Config => {
            let record = d.dir.join("record");
            let script = d.dir.join("bin").join("wrap.sh");
            std::fs::write(
                &script,
                format!(
                    "printf '%s|' \"$@\" >> '{rec}'\necho >> '{rec}'\npwd >> '{rec}.cwd'\n\
                     [ -n \"$PATH\" ] && echo path >> '{rec}.env'\nexec '{exe}' \"$@\"\n",
                    rec = record.display(),
                    exe = d.exe.display()
                ),
            )
            .unwrap();
            let mut table = toml::Table::new();
            table.insert("command".into(), "/bin/sh".into());
            let args: Vec<toml::Value> = [script.to_string_lossy().as_ref(), "one", "two words"]
                .iter()
                .map(|a| toml::Value::from(*a))
                .collect();
            table.insert("args".into(), args.into());
            ([("demo-clock".to_string(), table)].into(), Vec::new())
        }
    };
    let token = external::install(&env, &tables, &devs).expect("the clock program registers");
    let problems = external::problems(&token, lang);
    assert!(problems.is_empty(), "{problems:?}");
    let guard = Guard {
        token: token.clone(),
        exe: d.exe.clone(),
    };
    let mut s = session_of(&d, &token, tables, lang);
    s.select("demo.konoma-demo");
    s.enter();
    s.draw();
    Demo {
        s,
        guard,
        _dir: d.dir,
    }
}

/// Opens `samples/demo.konoma-demo` with the built program loaded the way `--plugin` loads it.
fn open_demo_in(name: &str, lang: Lang) -> Demo {
    open_demo_via(name, lang, Via::Dev)
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
    // `--serve` is added by the host: once, and last.
    assert_eq!(alive[0].matches("--serve").count(), 1, "{alive:?}");
    assert!(
        alive[0].ends_with(&format!("{} --serve", d.guard.exe.display())),
        "{alive:?}"
    );
    let reg = crate::plugins::shared_registry(&d.app.cfg.plugin_disabled());
    assert_eq!(
        reg.origin("demo-clock"),
        Some(konoma_plugin::registry::PluginOrigin::User)
    );
}

#[test]
fn e2e_demo_clock_a_configured_command_gets_the_users_args_then_serve() {
    let d = open_demo_via("demo_clock_cfg", Lang::En, Via::Config);
    d.see("konoma canvas demo");
    d.see("0 s");
    let alive = processes_of(&d.guard.exe);
    assert_eq!(alive.len(), 1, "{alive:?}");
    assert!(
        alive[0].ends_with(&format!("{} one two words --serve", d.guard.exe.display())),
        "the program got the user's args, then --serve once: {alive:?}"
    );
    // What the script saw, every start (the manifest was asked for first, then the serving child).
    let record = std::fs::read_to_string(d._dir.join("record")).unwrap();
    assert!(!record.is_empty());
    for line in record.lines().filter(|l| !l.is_empty()) {
        assert_eq!(line, "one|two words|--serve|", "{record:?}");
    }
    let work = canon(&d._dir.join("work"));
    let cwd = std::fs::read_to_string(d._dir.join("record.cwd")).unwrap();
    for line in cwd.lines() {
        assert_eq!(
            Path::new(line),
            work,
            "the child works in the host's private directory"
        );
    }
    let env = std::fs::read_to_string(d._dir.join("record.env")).unwrap();
    assert!(
        env.starts_with("path"),
        "the child inherits the environment: {env:?}"
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

// ---- what only a real process shows ---------------------------------------------------------------

/// `[plugins.demo-clock] command = "/bin/sh"`, `args = ["-c", <script>, "sh"]`: a program of the
/// test's own that is not a plugin at all.
fn sh_tables(script: &str) -> crate::plugins::PluginTables {
    let mut table = toml::Table::new();
    table.insert("command".into(), "/bin/sh".into());
    let args: Vec<toml::Value> = ["-c", script, "sh"]
        .iter()
        .map(|a| toml::Value::from(*a))
        .collect();
    table.insert("args".into(), args.into());
    [("demo-clock".to_string(), table)].into()
}

#[test]
fn e2e_demo_clock_a_program_that_exits_at_once_is_not_loaded_and_says_why() {
    let d = dirs_of("demo_clock_exits");
    let env = env_of(&d, Lang::En);
    let tables = sh_tables("echo 'usage: not a plugin' >&2; exit 3");
    let token = external::install(&env, &tables, &[]).expect("a set is installed");
    assert!(
        external::plugins_of(&token).is_empty(),
        "nothing registered"
    );
    let en = external::problems(&token, Lang::En);
    assert_eq!(en.len(), 1, "{en:?}");
    assert!(en[0].contains("not loaded"), "{en:?}");
    assert!(
        en[0].contains("usage: not a plugin"),
        "what the child wrote to stderr is in the reason: {en:?}"
    );
    external::shutdown_set(&token);
}

#[test]
fn e2e_demo_clock_a_silent_program_is_given_up_on_and_really_killed() {
    let d = dirs_of("demo_clock_silent");
    let mut env = env_of(&d, Lang::En);
    env.handshake_limit = std::time::Duration::from_millis(500);
    // A distinctive duration, so that only this test's child matches the needle.
    let tables = sh_tables("exec sleep 4137");
    let token = external::install(&env, &tables, &[]).expect("a set is installed");
    assert!(external::plugins_of(&token).is_empty());
    let en = external::problems(&token, Lang::En);
    assert!(en[0].contains("500ms"), "{en:?}");
    // Only a command line that is exactly the program (not a shell that merely names it).
    let left: Vec<String> = processes_of(Path::new("sleep 4137"))
        .into_iter()
        .filter(|l| l.split_once(' ').map(|x| x.1.trim()) == Some("sleep 4137"))
        .collect();
    assert!(left.is_empty(), "the silent child is gone: {left:?}");
    external::shutdown_set(&token);
}

#[test]
fn e2e_demo_clock_an_idle_child_is_reaped_and_a_new_process_serves_the_next_open() {
    let mut d = open_demo("demo_clock_reap");
    let first = processes_of(&d.guard.exe);
    assert_eq!(first.len(), 1, "{first:?}");
    d.esc();
    let idle_after = konoma_plugin::proc::client::Limits::default().idle_after;
    let later = std::time::Instant::now() + idle_after + std::time::Duration::from_secs(1);
    assert_eq!(external::reap_set_at(&d.guard.token, later), 1);
    let gone = processes_of(&d.guard.exe);
    assert!(
        gone.is_empty(),
        "the idle child is gone from `ps`: {gone:?}"
    );
    d.select("demo.konoma-demo");
    d.enter();
    d.draw();
    d.see("konoma canvas demo");
    let again = processes_of(&d.guard.exe);
    assert_eq!(again.len(), 1, "a new child serves: {again:?}");
}

#[test]
fn e2e_demo_clock_the_exit_cleanup_stops_the_real_child() {
    // Process-wide and one-way (the shutting-down flag), so it runs in a process of its own.
    const ENV: &str = "KONOMA_TEST_CLOCK_EXIT_CHILD";
    if std::env::var_os(ENV).is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "e2e_tests::demo_clock::e2e_demo_clock_the_exit_cleanup_stops_the_real_child",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(ENV, "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let d = open_demo("demo_clock_exit");
    d.see("konoma canvas demo");
    assert_eq!(processes_of(&d.guard.exe).len(), 1);
    crate::preview::command::exit_cleanup();
    assert!(external::is_shutting_down());
    let left = processes_of(&d.guard.exe);
    assert!(left.is_empty(), "exit_cleanup left the child: {left:?}");
}
