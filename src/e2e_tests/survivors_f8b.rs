//! End-to-end tests added after the second stage-2 mutation audit. Driven through the real
//! `handle_key`.

use super::*;
use crate::plugins::test_plugins::{self, Hexdump, Variant};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A session on `a.bin` with the test hexdump plugin (`w` = width, optionally declared as
/// writing) and `read_only` set in `[plugins.hexdump]` as given (`None` = not written).
fn hexdump_session(
    name: &str,
    writes: bool,
    read_only: Option<toml::Value>,
) -> (Sim, Arc<AtomicUsize>, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    std::fs::write(dir.join("a.bin"), (0u8..20).collect::<Vec<_>>()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let marker = test_plugins::register(
        name,
        Arc::new(Hexdump(Variant {
            writes,
            calls: calls.clone(),
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    if let Some(v) = read_only {
        let mut t = toml::Table::new();
        t.insert("read_only".into(), v);
        cfg.plugins.insert("hexdump".into(), t);
    }
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.bin");
    s.enter();
    (s, calls, dir)
}

/// `read_only = true` stops what writes and only that: an action that does not write still runs.
#[test]
fn e2e_read_only_stops_writing_actions_and_leaves_the_others() {
    let (mut s, calls, _dir) = hexdump_session("f8b_ro_nowrite", false, Some(true.into()));
    s.key('w');
    assert_eq!(calls.load(Ordering::SeqCst), 1, "a reading action runs");
    assert!(
        !s.app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("read-only")),
        "{:?}",
        s.app.flash
    );
    let (mut s, calls, _dir) = hexdump_session("f8b_ro_write", true, Some(true.into()));
    s.key('w');
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a writing action is stopped"
    );
    assert!(s
        .app
        .flash
        .as_deref()
        .is_some_and(|f| f.contains("read-only")));
}

/// `read_only = false` (or nothing written) is not read-only: a writing action runs.
#[test]
fn e2e_a_writing_action_runs_unless_read_only_is_true() {
    for (name, ro) in [
        ("f8b_ro_false", Some(toml::Value::Boolean(false))),
        ("f8b_ro_unset", None),
        // Only the boolean `true` counts; a string is reported at startup and does not stop writes.
        ("f8b_ro_string", Some(toml::Value::String("true".into()))),
    ] {
        let (mut s, calls, _dir) = hexdump_session(name, true, ro);
        s.key('w');
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{name}");
        assert!(s.app.flash.is_none(), "{name}: {:?}", s.app.flash);
    }
}

/// The question itself, asked of the app: only `read_only = true` of that plugin's own table.
#[test]
fn plugin_is_read_only_is_true_only_for_true_in_the_plugins_own_table() {
    let (s, _, _dir) = hexdump_session("f8b_ro_probe_true", true, Some(true.into()));
    assert!(s.app.plugin_is_read_only("hexdump"));
    assert!(!s.app.plugin_is_read_only("csv"), "another plugin's table");
    let (s, _, _dir) = hexdump_session("f8b_ro_probe_false", true, Some(false.into()));
    assert!(!s.app.plugin_is_read_only("hexdump"));
    let (s, _, _dir) = hexdump_session("f8b_ro_probe_unset", true, None);
    assert!(!s.app.plugin_is_read_only("hexdump"));
}
