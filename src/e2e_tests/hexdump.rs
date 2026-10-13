//! The hello-world plugin of the design (4-10) as a **user plugin in the host**: a `hexdump` that
//! declares the files it takes (`*.bin`), that it shows text (`shows(ViewKind::Text)`: without it
//! the host attaches no screen, says so at startup and on the file) and one action (`w`, "width")
//! gets its key, its footer entry, its row in
//! the `?` help and its place in the `Space` menu from the declaration alone, and the key re-draws
//! the view. Driven through the real `handle_key`; the plugin is registered the way the stage-3
//! registration will (`plugins::test_plugins`), never in the shipped registry.

use super::*;
use crate::keymap::{Action, Context, LeaderId};
use crate::ops::{self, menu::View};
use crate::plugins::test_plugins::{self, Hexdump, Variant};
use crate::plugins::PluginAct;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A sandbox with `a.bin` (20 bytes: two rows of 16, then 3 rows of 8) and `small.bin` (4 bytes).
fn files(name: &str) -> crate::test_support::TmpDir {
    let dir = sandbox(name);
    std::fs::write(dir.join("a.bin"), (0u8..20).collect::<Vec<_>>()).unwrap();
    std::fs::write(dir.join("small.bin"), [1u8, 2, 3, 4]).unwrap();
    dir
}

/// A session in English with the plugin registered under a name of this test's own, on `a.bin`.
fn open(
    name: &str,
    variant: Variant,
    keys: &[(&str, &str, &str)],
    file: &str,
) -> (Sim, crate::test_support::TmpDir) {
    let dir = files(name);
    let marker = test_plugins::register(name, Arc::new(Hexdump(variant)));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    for (ctx, key, action) in keys {
        cfg.keys
            .surfaces
            .entry((*ctx).to_string())
            .or_default()
            .insert((*key).to_string(), (*action).to_string());
    }
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select(file);
    s.enter();
    (s, dir)
}

fn width() -> PluginAct {
    PluginAct::new("hexdump", "width")
}

/// The footer line of the drawn screen.
fn footer(s: &Sim) -> String {
    s.screen().lines().last().unwrap_or_default().to_string()
}

fn space_items(s: &Sim) -> Vec<(String, Action, String)> {
    ops::menu::menu(&s.app, Context::Text, LeaderId::Actions, View::Open)
        .map(|m| {
            m.items
                .into_iter()
                .map(|i| (i.key.label(), i.action, i.label))
                .collect()
        })
        .unwrap_or_default()
}

/// The whole story of the design's hello world: the file opens in the plugin, the footer names
/// the key, the help lists it, the key changes the view, and `Space` offers it.
#[test]
fn e2e_hexdump_declaration_alone_gives_footer_help_key_and_menu() {
    let (mut s, _dir) = open("e2e_hexdump_story", Variant::default(), &[], "a.bin");
    assert_eq!(s.app.key_context(), Some(Context::Text));
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    s.dont_see("08 09 0a 0b 0c 0d 0e 0f 10"); // 16 bytes per row at first

    // (a) the footer.
    assert!(footer(&s).contains("w:width"), "footer: {}", footer(&s));

    // (b) the help.
    s.key('?');
    s.see("hexdump");
    s.see("Switch 8/16 bytes per row");
    s.esc();

    // (d) the Space menu offers it, under the key it already has.
    let items = space_items(&s);
    let width_item = items
        .iter()
        .find(|(_, a, _)| *a == Action::Plugin(width()))
        .unwrap_or_else(|| panic!("no width item: {items:?}"));
    assert_eq!(width_item.0, "w");
    assert_eq!(width_item.2, "width");

    // (c) the key re-draws the view: 16 bytes per row become 8.
    s.key('w');
    s.see("00 01 02 03 04 05 06 07");
    s.dont_see("00 01 02 03 04 05 06 07 08");
    s.see("10 11 12 13");
    s.key('w');
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
}

/// The same action from the `Space` menu, by the letter the menu shows.
#[test]
fn e2e_hexdump_runs_from_the_space_menu() {
    let (mut s, _dir) = open("e2e_hexdump_menu", Variant::default(), &[], "a.bin");
    s.key(' ');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Actions));
    s.see("width");
    s.key('w');
    assert_eq!(s.app.pending_leader, None);
    s.dont_see("00 01 02 03 04 05 06 07 08");
}

/// (e) `[keys.hexdump] W = "width"` moves the key; the footer and the help follow, and the old key
/// is free (it does nothing).
#[test]
fn e2e_hexdump_key_can_be_rebound_and_hints_follow() {
    let (mut s, _dir) = open(
        "e2e_hexdump_rebind",
        Variant::default(),
        &[("hexdump", "W", "width")],
        "a.bin",
    );
    assert!(footer(&s).contains("W:width"), "footer: {}", footer(&s));
    assert!(!footer(&s).contains("w:width"), "footer: {}", footer(&s));
    s.key('?');
    let help = s.screen();
    let row = help
        .lines()
        .find(|l| l.contains("Switch 8/16 bytes per row"))
        .unwrap_or_else(|| panic!("no help row:\n{help}"));
    assert!(row.contains('W'), "help row: {row}");
    s.esc();
    // The old key is the host's again (`w` is nothing here); the new one works.
    s.key('w');
    s.see("00 01 02 03 04 05 06 07 08");
    s.key('W');
    s.dont_see("00 01 02 03 04 05 06 07 08");
    // The Space menu uses the new key as well.
    let items = space_items(&s);
    assert!(
        items
            .iter()
            .any(|(k, a, _)| k == "W" && *a == Action::Plugin(width())),
        "{items:?}"
    );
    assert!(s
        .app
        .startup_flash(None, 300)
        .is_none_or(|f| !f.contains("hexdump")));
}

/// `[keys.hexdump] w = "noop"` takes the key away; the action stays in the menu.
#[test]
fn e2e_hexdump_key_can_be_taken_away() {
    let (mut s, _dir) = open(
        "e2e_hexdump_noop",
        Variant::default(),
        &[("hexdump", "w", "noop")],
        "a.bin",
    );
    assert!(!footer(&s).contains("width"), "footer: {}", footer(&s));
    s.key('w');
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    let items = space_items(&s);
    let item = items
        .iter()
        .find(|(_, a, _)| *a == Action::Plugin(width()))
        .unwrap_or_else(|| panic!("the action is still in the menu: {items:?}"));
    s.key(' ');
    s.key(item.0.chars().next().unwrap());
    s.dont_see("00 01 02 03 04 05 06 07 08");
}

/// (f) While the session says an action does not work, no hint names it (footer, help, menu) and
/// its key does nothing: the session is not even asked.
#[test]
fn e2e_hexdump_disabled_action_is_not_offered_and_does_not_act() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut s, _dir) = open(
        "e2e_hexdump_gated",
        Variant {
            gate_len: 8,
            calls: calls.clone(),
            ..Variant::default()
        },
        &[],
        "small.bin",
    );
    s.see("01 02 03 04");
    assert!(!s.app.plugin_act_enabled(width()));
    assert!(!footer(&s).contains("width"), "footer: {}", footer(&s));
    s.key('?');
    s.dont_see("Switch 8/16 bytes per row");
    s.esc();
    assert!(
        space_items(&s)
            .iter()
            .all(|(_, a, _)| *a != Action::Plugin(width())),
        "{:?}",
        space_items(&s)
    );
    s.key('w');
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the action did not run");
    // The same plugin on a file where it works: everything is there.
    s.esc();
    s.select("a.bin");
    s.enter();
    assert!(s.app.plugin_act_enabled(width()));
    assert!(footer(&s).contains("w:width"), "footer: {}", footer(&s));
    s.key('w');
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// (g) A key of layers 0 to 3 (`j`) is refused: the action is registered without it, the startup
/// message says so, and the action is still in the `Space` menu. The key keeps its host meaning.
#[test]
fn e2e_hexdump_reserved_key_is_dropped_and_told_but_the_menu_keeps_the_action() {
    let (mut s, _dir) = open(
        "e2e_hexdump_reserved",
        Variant {
            extra_acts: true,
            ..Variant::default()
        },
        &[],
        "a.bin",
    );
    let note = s
        .app
        .startup_flash(None, 400)
        .expect("a startup message about the refused key");
    assert!(
        note.contains("jump") && note.contains("`j`") && note.contains("reserved"),
        "{note}"
    );
    // `j` scrolls (the host's), it does not run the plugin's action.
    let jump = PluginAct::new("hexdump", "jump");
    assert_eq!(
        s.app
            .plugin_action_for_key(Context::Text, crate::keymap::KeyPress::ch('j')),
        None
    );
    assert!(
        !footer(&s).contains("jump"),
        "no key to name: {}",
        footer(&s)
    );
    // It is in the menu, and runs from there.
    let items = space_items(&s);
    let item = items
        .iter()
        .find(|(_, a, _)| *a == Action::Plugin(jump))
        .unwrap_or_else(|| panic!("jump in the menu: {items:?}"));
    assert_eq!(item.2, "jump");
    s.key(' ');
    s.key(item.0.chars().next().unwrap());
    assert_eq!(s.app.flash.as_deref(), Some("did jump"));
    // `x` was accepted (nothing reserves it in text) and is bound.
    s.key('x');
    assert_eq!(s.app.flash.as_deref(), Some("did dump"));
}

/// A writing action is stopped, with the reason, for a plugin set to `read_only = true`.
#[test]
fn e2e_hexdump_writing_action_respects_read_only() {
    let dir = files("e2e_hexdump_readonly");
    let calls = Arc::new(AtomicUsize::new(0));
    let marker = test_plugins::register(
        "e2e_hexdump_readonly",
        Arc::new(Hexdump(Variant {
            writes: true,
            calls: calls.clone(),
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut t = toml::Table::new();
    t.insert("read_only".into(), toml::Value::Boolean(true));
    cfg.plugins.insert("hexdump".into(), t);
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.bin");
    s.enter();
    s.key('w');
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        s.app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("read-only")),
        "{:?}",
        s.app.flash
    );
}

/// The hexdump written against the public API is the one `crates/konoma-plugin/tests/hexdump.rs`
/// checks without a host: the footer and help names come from the same declaration in both.
#[test]
fn e2e_hexdump_the_footer_names_come_from_the_declaration_in_both_languages() {
    let dir = files("e2e_hexdump_ja");
    let marker = test_plugins::register("e2e_hexdump_ja", Arc::new(Hexdump(Variant::default())));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "ja".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.bin");
    s.enter();
    assert!(footer(&s).contains("w:幅"), "footer: {}", footer(&s));
    s.key('?');
    // The wide glyphs of the test terminal are drawn with spaces between them: look for the
    // part that is ASCII, on the row of the plugin's key.
    let help = s.screen();
    assert!(
        help.lines()
            .any(|l| l.contains("  w ") && l.contains("8/16")),
        "{help}"
    );
}

/// The same story with the **worker** in the loop, as `main` runs it: `open`, the action and the
/// view that follows `Outcome::Redraw` all run off the UI thread, and the answer of `enabled`
/// comes back with the session.
#[test]
fn e2e_hexdump_act_and_redraw_through_the_worker() {
    let dir = files("e2e_hexdump_worker");
    let marker =
        test_plugins::register("e2e_hexdump_worker", Arc::new(Hexdump(Variant::default())));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir), cfg).with_async_plugins();
    s.select("a.bin");
    s.enter();
    for _ in 0..5 {
        if s.screen().contains("00 01 02 03") {
            break;
        }
        s.drain_plugin();
    }
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    assert!(s.app.plugin_act_enabled(width()));
    assert!(footer(&s).contains("w:width"), "footer: {}", footer(&s));
    s.key('w');
    for _ in 0..5 {
        if !s.screen().contains("00 01 02 03 04 05 06 07 08") {
            break;
        }
        s.drain_plugin();
    }
    s.dont_see("00 01 02 03 04 05 06 07 08");
    s.see("10 11 12 13");
    // The session is back in the show: the action works again.
    assert!(s.app.plugin_act_enabled(width()));
}

/// The host and the plugin's own tests (`konoma_plugin::testing::check_hints`) decide the same
/// thing: which declared actions are shown. Compared on a file where the session disables one
/// action and on one where it does not.
#[test]
fn e2e_hexdump_the_host_shows_what_the_shared_rule_says() {
    use konoma_plugin::hints::hints;
    use konoma_plugin::testing::{check_hints, open_file};
    let variant = Variant {
        extra_acts: true,
        gate_len: 8,
        ..Variant::default()
    };
    let (mut s, dir) = open("e2e_hexdump_rule", variant.clone(), &[], "small.bin");
    for file in ["small.bin", "a.bin"] {
        s.esc();
        s.select(file);
        s.enter();
        let plugin = Hexdump(variant.clone());
        let session = open_file(&plugin, &dir.join(file)).unwrap();
        assert!(check_hints(&plugin, session.as_ref())
            .shown_while_disabled
            .is_empty());
        use konoma_plugin::Plugin;
        let by_rule: Vec<String> = hints(&plugin.manifest(), &|id| session.enabled(id))
            .into_iter()
            .map(|h| h.act)
            .collect();
        let in_menu: Vec<String> = space_items(&s)
            .into_iter()
            .filter_map(|(_, a, _)| match a {
                Action::Plugin(p) => Some(p.act().to_string()),
                _ => None,
            })
            .collect();
        let mut a = by_rule.clone();
        let mut b = in_menu.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b, "{file}: rule {by_rule:?} / menu {in_menu:?}");
        // The footer names exactly the ones with a footer entry and a key.
        let has_width = footer(&s).contains("w:width");
        assert_eq!(has_width, by_rule.iter().any(|x| x == "width"), "{file}");
    }
}

/// The hello world as it was first written, without `shows`: the host would attach no screen, so
/// it says so at startup (naming the plugin) and, on the file, on the screen (not a silent blank).
#[test]
fn e2e_hexdump_without_shows_is_named_at_startup_and_on_screen() {
    let (s, _dir) = open(
        "e2e_hexdump_noshows",
        Variant {
            no_shows: true,
            ..Variant::default()
        },
        &[],
        "a.bin",
    );
    let note = s
        .app
        .startup_flash(None, 400)
        .expect("a startup message about the missing shows");
    assert!(
        note.contains("hexdump-ns") && note.contains("shows"),
        "{note}"
    );
    s.see("returned a text view but declares no `shows`");
    s.see("hexdump-ns");
}

/// A plugin that answers with a display type it did not declare is told on the screen, naming the
/// plugin, what it returned and what it declared (not a silent blank).
#[test]
fn e2e_hexdump_a_view_of_an_undeclared_type_is_explained_on_screen() {
    let (s, _dir) = open(
        "e2e_hexdump_wrong",
        Variant {
            wrong_view: true,
            ..Variant::default()
        },
        &[],
        "a.bin",
    );
    s.see("plugin hexdump-wrong returned a table view but declares only text");
    // In Japanese too.
    let dir = files("e2e_hexdump_wrong_ja");
    let marker = test_plugins::register(
        "e2e_hexdump_wrong_ja",
        Arc::new(Hexdump(Variant {
            wrong_view: true,
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "ja".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.bin");
    s.enter();
    // (The test screen spaces out wide characters: read the failure itself.)
    assert_eq!(
        s.app
            .plugin_failure()
            .map(|f| f.message.get("ja").to_string()),
        Some(
            "プラグイン hexdump-wrong は table の表示を返しましたが、text しか宣言していません"
                .to_string()
        )
    );
}

/// While a job holds the session (the key was just pressed and the worker has not answered), a
/// second press would be ignored, so the hint and the gate are off: they are shown only while the
/// key acts.
#[test]
fn e2e_hexdump_the_hint_is_off_while_the_action_is_running() {
    let dir = files("e2e_hexdump_running");
    // The job is held on the worker until the test lets it go: the host takes an answer that is
    // already there, so a job that ends at once would not be "running" when the screen is read.
    let latch = Arc::new(test_plugins::Latch::default());
    let marker = test_plugins::register(
        "e2e_hexdump_running",
        Arc::new(Hexdump(Variant {
            hold: Some(Arc::clone(&latch)),
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir), cfg).with_async_plugins();
    s.select("a.bin");
    s.enter();
    for _ in 0..5 {
        if s.screen().contains("00 01 02 03") {
            break;
        }
        s.drain_plugin();
    }
    assert!(footer(&s).contains("w:width"));
    s.key('w');
    // The answer has not been applied: the session is out with the job.
    assert!(!s.app.plugin_act_enabled(width()), "running");
    assert!(!footer(&s).contains("w:width"), "footer: {}", footer(&s));
    latch.open();
    for _ in 0..5 {
        if s.app.plugin_act_enabled(width()) {
            break;
        }
        s.drain_plugin();
    }
    assert!(s.app.plugin_act_enabled(width()));
    assert!(footer(&s).contains("w:width"), "footer: {}", footer(&s));
}

/// Another media job moving the generation on while an action of the plugin is out on the worker
/// makes the action's answer stale. The answer carries the session, which belongs to the show: it
/// goes back to the show (the keys keep working), only the stale view and outcome are dropped.
#[test]
fn e2e_hexdump_a_stale_answer_gives_the_session_back() {
    let dir = files("e2e_hexdump_stale");
    // The action is held on the worker until the generation has moved on, so its answer is stale
    // whatever the speed of the machine (an answer that is already there would be taken at once).
    let latch = Arc::new(test_plugins::Latch::default());
    let marker = test_plugins::register(
        "e2e_hexdump_stale",
        Arc::new(Hexdump(Variant {
            hold: Some(Arc::clone(&latch)),
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir), cfg).with_async_plugins();
    s.select("a.bin");
    s.enter();
    for _ in 0..5 {
        if s.screen().contains("00 01 02 03") {
            break;
        }
        s.drain_plugin();
    }
    s.key('w');
    s.app.another_media_job_starts_for_test();
    latch.open();
    // The stale answer hands the session back and its redraw is asked for again.
    for _ in 0..5 {
        if s.app.plugin_act_enabled(width()) && !s.screen().contains("00 01 02 03 04 05 06 07 08") {
            break;
        }
        s.drain_plugin();
    }
    assert!(s.app.plugin_act_enabled(width()), "the session is lost");
    s.dont_see("00 01 02 03 04 05 06 07 08");
    assert!(footer(&s).contains("w:width"), "footer: {}", footer(&s));
    // And the key still acts: width 8 became 16 again.
    s.key('w');
    for _ in 0..5 {
        if s.screen().contains("00 01 02 03 04 05 06 07 08") {
            break;
        }
        s.drain_plugin();
    }
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    assert!(s.app.plugin_act_enabled(width()));
}

/// An answer that comes late from a job on one file must not give its session to the show of
/// another file of the same plugin. The action on `a.bin` is held on the worker; the preview moves
/// to `small.bin`, whose own action is held too (its show has no session); then the answer of
/// `a.bin` arrives first. Its session is not `small.bin`'s: the show of `small.bin` stays without
/// one until its own answer comes, and its keys then act on its own bytes.
#[test]
fn e2e_hexdump_a_late_answer_of_another_file_does_not_give_its_session_to_this_show() {
    let dir = files("e2e_hexdump_late_other_file");
    let latch_a = Arc::new(test_plugins::Latch::default());
    let latch_small = Arc::new(test_plugins::Latch::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let marker = test_plugins::register(
        "e2e_hexdump_late_other_file",
        Arc::new(Hexdump(Variant {
            hold: Some(Arc::clone(&latch_a)),
            hold_small: Some(Arc::clone(&latch_small)),
            calls: Arc::clone(&calls),
            ..Variant::default()
        })),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir), cfg).with_async_plugins();
    s.select("a.bin");
    s.enter();
    for _ in 0..5 {
        if s.screen().contains("00 01 02 03") {
            break;
        }
        s.drain_plugin();
    }
    // The action on a.bin is out on the worker, held.
    s.key('w');
    assert!(!s.app.plugin_act_enabled(width()));
    // The preview moves to small.bin, which opens and is ready.
    s.esc();
    s.select("small.bin");
    s.enter();
    for _ in 0..5 {
        if s.screen().contains("01 02 03 04") {
            break;
        }
        s.drain_plugin();
    }
    s.see("01 02 03 04");
    assert!(s.app.plugin_act_enabled(width()), "small.bin is ready");
    // Its own action is out on the worker too: its show holds no session.
    s.key('w');
    assert!(!s.app.plugin_act_enabled(width()));
    // The answer of a.bin comes first.
    latch_a.open();
    s.drain_plugin();
    assert!(
        !s.app.plugin_act_enabled(width()),
        "the session of a.bin was put into the show of small.bin"
    );
    // small.bin's own answer ends the wait; its keys act on its own bytes (4 bytes at width 8
    // after the held toggle, redrawn).
    latch_small.open();
    for _ in 0..5 {
        if s.app.plugin_act_enabled(width()) {
            break;
        }
        s.drain_plugin();
    }
    assert!(s.app.plugin_act_enabled(width()));
    s.see("01 02 03 04");
    s.dont_see("00 01 02 03 04 05 06 07");
}
