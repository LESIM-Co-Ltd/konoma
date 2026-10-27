//! Plugins that are programs of their own (stage 3): registered from `[plugins.<id>] command` and
//! `--plugin`, started as child processes, cached by manifest. The child is a thread over socket
//! pairs (`plugins::external_testkit`), so the whole path (handshake, cache, registration, the
//! read scope, timeouts, stopping) runs through the real host code without a real process.

use super::*;
use crate::plugins::external::{self, Env};
use crate::plugins::external_testkit::{table, Lab, Mode, Rig, PATIENCE};
use crate::plugins::{shared_registry, PluginTables};
use konoma_plugin::registry::PluginOrigin;
use konoma_plugin::Plugin;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn tables(entries: &[(&str, toml::Table)]) -> PluginTables {
    entries
        .iter()
        .map(|(id, t)| ((*id).to_string(), t.clone()))
        .collect()
}

/// A session over the lab's root with the given external set loaded, in English.
fn session(lab: &Lab, token: Option<String>, tables: PluginTables) -> Sim {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg.plugins = tables;
    cfg.external_plugins = token;
    Sim::with_config(&lab.root(), cfg)
}

fn open_file(s: &mut Sim, name: &str) {
    s.select(name);
    s.enter();
}

fn write_xd(lab: &Lab, name: &str, body: &str) {
    std::fs::write(lab.root().join(name), body).unwrap();
}

fn registry_of(s: &Sim) -> std::sync::Arc<konoma_plugin::registry::Registry> {
    shared_registry(&s.app.cfg.plugin_disabled())
}

fn png(path: &std::path::Path) {
    image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 10, 10, 255]))
        .save(path)
        .unwrap();
}

// ---- registration ------------------------------------------------------------------------------

#[test]
fn e2e_ext_a_configured_program_is_registered_as_a_user_plugin_and_shows_its_file() {
    let lab = Lab::new("reg");
    let rig = Rig::new("ext-reg");
    lab.program("ext-reg-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[(
        "ext-reg",
        table(
            "ext-reg-prog",
            &[("k", toml::Value::String("from-config".into()))],
        ),
    )]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    assert!(token.is_some());
    let mut s = session(&lab, token, t);

    // Registered the way a user's plugin is: the read scope and the worker rule apply to it.
    let reg = registry_of(&s);
    assert_eq!(reg.origin("ext-reg"), Some(PluginOrigin::User));
    assert!(reg.manifest("ext-reg").is_some());
    // The standard plugins are untouched.
    assert_eq!(reg.origin("markdown"), Some(PluginOrigin::Standard));

    open_file(&mut s, "a.xd");
    s.see("child says hello");
    // The settings of the table (not the host's keys) reached the child, with the language.
    s.see("k=Some(Str(\"from-config\"))");
    s.see("lang=en");
    // The read scope reached the child: the tab's root is in it.
    let root = lab.root();
    let flat: String = s
        .screen()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '│' && *c != '█')
        .collect();
    let want: String = root
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(flat.contains(&want), "scope lacks the tab root: {flat}");
    // The handshake started the child, and the first open used that same one.
    assert_eq!(rig.launches(), 1);
}

#[test]
fn e2e_ext_the_host_keys_command_args_and_enabled_are_not_sent_as_settings() {
    let lab = Lab::new("keys");
    let rig = Rig::new("ext-keys");
    lab.program("ext-keys-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[(
        "ext-keys",
        table(
            "ext-keys-prog",
            &[
                (
                    "args",
                    toml::Value::Array(vec![toml::Value::String("--x".into())]),
                ),
                ("enabled", toml::Value::Boolean(true)),
                ("k", toml::Value::String("v".into())),
            ],
        ),
    )]);
    let settings = crate::plugins::settings_of(&t["ext-keys"]);
    assert!(settings.get("command").is_none());
    assert!(settings.get("args").is_none());
    assert!(settings.get("enabled").is_none());
    assert!(settings.get("k").is_some());
    let token = external::install(&lab.env(&rig), &t, &[]);
    let mut s = session(&lab, token, t);
    open_file(&mut s, "a.xd");
    s.see("k=Some(Str(\"v\"))");
}

#[test]
fn e2e_ext_nothing_asked_for_loads_nothing_and_starts_nothing() {
    let lab = Lab::new("none");
    let rig = Rig::new("ext-none");
    assert!(external::install(&lab.env(&rig), &PluginTables::new(), &[]).is_none());
    assert_eq!(rig.launches(), 0);
    assert!(lab.cache_files().is_empty());
}

#[test]
fn e2e_ext_an_unrelated_plugin_table_asks_for_no_program() {
    let lab = Lab::new("unrelated");
    let rig = Rig::new("ext-unrelated");
    let mut md = toml::Table::new();
    md.insert("enabled".into(), toml::Value::Boolean(false));
    let t = tables(&[("markdown", md)]);
    assert!(external::install(&lab.env(&rig), &t, &[]).is_none());
    assert_eq!(rig.launches(), 0);
}

#[test]
fn e2e_ext_enabled_false_keeps_the_program_unloaded_and_unstarted() {
    let lab = Lab::new("off");
    let rig = Rig::new("ext-off");
    lab.program("ext-off-prog", "v1");
    let t = tables(&[(
        "ext-off",
        table("ext-off-prog", &[("enabled", toml::Value::Boolean(false))]),
    )]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    assert!(token.is_none(), "nothing else was asked for");
    assert_eq!(rig.launches(), 0);
    assert!(lab.cache_files().is_empty());

    // Next to a program that is on, the one that is off is still absent.
    let rig2 = Rig::new("ext-on");
    lab.program("ext-on-prog", "v1");
    let mut both = t.clone();
    both.insert("ext-on".into(), table("ext-on-prog", &[]));
    let token = external::install(&lab.env(&rig2), &both, &[]);
    let s = session(&lab, token, both);
    let reg = registry_of(&s);
    assert!(reg.manifest("ext-on").is_some());
    assert!(reg.manifest("ext-off").is_none());
    assert_eq!(rig.launches(), 0);
}

// ---- the manifest cache --------------------------------------------------------------------------

#[test]
fn e2e_ext_a_miss_asks_the_program_once_and_a_hit_starts_nothing_until_the_first_open() {
    let lab = Lab::new("cache");
    let rig = Rig::new("ext-cache");
    lab.program("ext-cache-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[("ext-cache", table("ext-cache-prog", &[]))]);

    // Miss: the child was asked for its manifest, which was saved.
    let _first = external::install(&lab.env(&rig), &t, &[]);
    assert_eq!(rig.launches(), 1);
    assert_eq!(lab.cache_files().len(), 1, "{:?}", lab.cache_files());

    // Hit: registered from the saved manifest, no child started.
    let rig2 = Rig::new("ext-cache");
    let token = external::install(&lab.env(&rig2), &t, &[]);
    assert_eq!(rig2.launches(), 0, "a cache hit starts nothing");
    let mut s = session(&lab, token, t.clone());
    assert!(registry_of(&s).manifest("ext-cache").is_some());
    assert_eq!(rig2.launches(), 0, "registering does not start it either");

    // The first open starts it.
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    assert_eq!(rig2.launches(), 1);
}

#[test]
fn e2e_ext_a_cache_entry_of_another_build_is_not_used() {
    let lab = Lab::new("cache_stamp");
    let rig = Rig::new("ext-stamp");
    let exe = lab.program("ext-stamp-prog", "v1");
    let t = tables(&[("ext-stamp", table("ext-stamp-prog", &[]))]);
    let _ = external::install(&lab.env(&rig), &t, &[]);
    assert_eq!(rig.launches(), 1);

    // The program is rebuilt (another size): its entry is for the old build.
    crate::test_support::write_executable(&exe, "v2 longer");
    let rig2 = Rig::new("ext-stamp");
    let _ = external::install(&lab.env(&rig2), &t, &[]);
    assert_eq!(rig2.launches(), 1, "the manifest is asked for again");

    // The entry of the new build is the one now kept: a third start is a hit.
    let rig3 = Rig::new("ext-stamp");
    let _ = external::install(&lab.env(&rig3), &t, &[]);
    assert_eq!(rig3.launches(), 0);
}

#[test]
fn e2e_ext_a_broken_cache_file_is_dropped_and_the_manifest_asked_for_again() {
    let garbage: &[&[u8]] = &[
        b"",
        b"not json at all",
        b"{\"format\":1}",
        b"{\"format\":1,\"path\":\"/x\",\"size\":1,\"mtime_s\":1,\"mtime_ns\":1,\"ino\":1,\"manifest\":{}}",
        b"\xff\xfe\x00 not utf-8",
        b"[1,2,3]",
    ];
    for (i, bad) in garbage.iter().enumerate() {
        let lab = Lab::new(&format!("cache_bad{i}"));
        let rig = Rig::new("ext-bad");
        lab.program("ext-bad-prog", "v1");
        let t = tables(&[("ext-bad", table("ext-bad-prog", &[]))]);
        let _ = external::install(&lab.env(&rig), &t, &[]);
        let files = lab.cache_files();
        assert_eq!(files.len(), 1);
        std::fs::write(&files[0], bad).unwrap();

        let rig2 = Rig::new("ext-bad");
        let token = external::install(&lab.env(&rig2), &t, &[]);
        assert_eq!(rig2.launches(), 1, "case {i}: asked again");
        let s = session(&lab, token, t.clone());
        assert!(registry_of(&s).manifest("ext-bad").is_some(), "case {i}");
        // The file now holds a good entry again.
        let rig3 = Rig::new("ext-bad");
        let _ = external::install(&lab.env(&rig3), &t, &[]);
        assert_eq!(rig3.launches(), 0, "case {i}: repaired");
    }
}

#[test]
fn e2e_ext_an_unwritable_cache_does_not_stop_the_plugin_from_loading() {
    let lab = Lab::new("cache_ro");
    let rig = Rig::new("ext-ro");
    lab.program("ext-ro-prog", "v1");
    let t = tables(&[("ext-ro", table("ext-ro-prog", &[]))]);
    let mut env = lab.env(&rig);
    // A file where the cache directory should be.
    let blocked = lab.dir.join("blocked");
    std::fs::write(&blocked, "x").unwrap();
    env.cache_dir = blocked.join("sub");
    let token = external::install(&env, &t, &[]);
    let s = session(&lab, token, t);
    assert!(registry_of(&s).manifest("ext-ro").is_some());
}

// ---- a rebuilt program -----------------------------------------------------------------------------

#[test]
fn e2e_ext_a_rebuilt_program_is_started_again_on_the_next_open() {
    let lab = Lab::new("rebuild");
    let rig = Rig::new("ext-rb");
    let exe = lab.program("ext-rb-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[("ext-rb", table("ext-rb-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    let mut s = session(&lab, token, t);
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    assert_eq!((rig.launches(), rig.kills()), (1, 0));

    // `r` on the same build re-reads the file through the same child.
    s.key('r');
    assert_eq!(
        (rig.launches(), rig.kills()),
        (1, 0),
        "not rebuilt: same child"
    );

    // The program is rebuilt: `r` stops the old child and starts one from the new build.
    crate::test_support::write_executable(&exe, "v2 is a longer program");
    s.key('r');
    s.see("child says hello");
    assert_eq!(rig.launches(), 2, "started again");
    assert_eq!(rig.kills(), 1, "the old child was stopped");

    // And the saved manifest follows the build: a restart finds a hit.
    let rig2 = Rig::new("ext-rb");
    let t2 = tables(&[("ext-rb", table("ext-rb-prog", &[]))]);
    let _ = external::install(&lab.env(&rig2), &t2, &[]);
    assert_eq!(rig2.launches(), 0);
}

// ---- collisions and failures ----------------------------------------------------------------------

#[test]
fn e2e_ext_an_id_of_a_standard_plugin_is_refused_and_said_at_start_up() {
    let lab = Lab::new("clash_std");
    let rig = Rig::new("markdown");
    lab.program("md-prog", "v1");
    let t = tables(&[("markdown", table("md-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).expect("something was asked for");
    let problems = external::problems(&token, crate::i18n::Lang::En);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("markdown") && problems[0].contains("already taken"));
    let s = session(&lab, Some(token), t);
    // The standard one is what is registered.
    assert_eq!(
        registry_of(&s).origin("markdown"),
        Some(PluginOrigin::Standard)
    );
    // The reason is in the start-up message.
    let flash = s.app.startup_flash(None, 400).unwrap_or_default();
    assert!(flash.contains("already taken"), "{flash}");
    assert!(external::plugins_of(s.app.cfg.external_plugins.as_deref().unwrap()).is_empty());
}

#[test]
fn e2e_ext_an_id_of_an_internal_piece_is_refused() {
    let lab = Lab::new("clash_int");
    for id in ["text", "command", "can-not-preview"] {
        let rig = Rig::new(id);
        lab.program("int-prog", "v1");
        let t = tables(&[(id, table("int-prog", &[]))]);
        let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
        assert!(external::plugins_of(&token).is_empty(), "{id}");
        assert_eq!(
            external::problems(&token, crate::i18n::Lang::En).len(),
            1,
            "{id}"
        );
    }
}

#[test]
fn e2e_ext_the_later_of_two_programs_with_one_id_is_refused() {
    let lab = Lab::new("clash_ext");
    let rig = Rig::new("ext-twice");
    let first = lab.program("first-prog", "v1");
    let second = lab.program("second-prog", "v2");
    let t = tables(&[("ext-twice", table("first-prog", &[]))]);
    // The config entry is read before the `--plugin` one.
    let token = external::install(&lab.env(&rig), &t, std::slice::from_ref(&second)).unwrap();
    let kept = external::plugins_of(&token);
    assert_eq!(kept.len(), 1);
    let problems = external::problems(&token, crate::i18n::Lang::En);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("--plugin") && problems[0].contains("already taken"));
    let _ = first;
    // The one that lost was stopped.
    assert!(rig.kills() >= 1);
}

#[test]
fn e2e_ext_a_program_whose_manifest_id_differs_from_its_table_is_refused() {
    let lab = Lab::new("id_mismatch");
    let rig = Rig::new("declared-id");
    lab.program("mm-prog", "v1");
    let t = tables(&[("other-id", table("mm-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    let problems = external::problems(&token, crate::i18n::Lang::En);
    assert!(
        problems[0].contains("declared-id") && problems[0].contains("other-id"),
        "{problems:?}"
    );
}

#[test]
fn e2e_ext_failures_to_load_are_not_registered_and_say_why_in_both_languages() {
    let lab = Lab::new("fails");
    // Not found.
    let rig = Rig::new("ext-missing");
    let t = tables(&[("ext-missing", table("no-such-program-anywhere", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    let en = external::problems(&token, crate::i18n::Lang::En);
    let ja = external::problems(&token, crate::i18n::Lang::Jp);
    assert!(
        en[0].contains("no-such-program-anywhere") && en[0].contains("not found"),
        "{en:?}"
    );
    assert!(
        ja[0].contains("no-such-program-anywhere") && ja[0].contains("読み込みません"),
        "{ja:?}"
    );
    assert_eq!(rig.launches(), 0);

    // A file that is not executable.
    let plain = lab.progs.join("plain-file");
    std::fs::write(&plain, "x").unwrap();
    let t = tables(&[("ext-plain", table("plain-file", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    assert!(external::problems(&token, crate::i18n::Lang::En)[0].contains("not executable"));

    // Does not speak the protocol.
    let rig = Rig::new("ext-garbage");
    *rig.mode.lock().unwrap() = Mode::Garbage;
    lab.program("garbage-prog", "v1");
    let t = tables(&[("ext-garbage", table("garbage-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    let en = external::problems(&token, crate::i18n::Lang::En);
    assert!(en[0].contains("did not start"), "{en:?}");
    assert!(lab.cache_files().is_empty(), "a failure is not cached");
    let s = session(&lab, Some(token), t);
    assert!(registry_of(&s).manifest("ext-garbage").is_none());
    let flash = s.app.startup_flash(None, 600).unwrap_or_default();
    assert!(flash.contains("did not start"), "{flash}");
}

#[test]
fn e2e_ext_a_program_that_never_answers_the_handshake_is_given_up_on_within_the_bound() {
    let lab = Lab::new("silent");
    let rig = Rig::new("ext-silent");
    *rig.mode.lock().unwrap() = Mode::Silent;
    lab.program("silent-prog", "v1");
    let t = tables(&[("ext-silent", table("silent-prog", &[]))]);
    let mut env = lab.env(&rig);
    env.handshake_limit = Duration::from_millis(300);
    let token = external::install(&env, &t, &[]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    let en = external::problems(&token, crate::i18n::Lang::En);
    assert!(
        en[0].contains("not loaded") || en[0].contains("did not start"),
        "{en:?}"
    );
    assert!(rig.kills() >= 1, "the silent child was killed");
    assert!(lab.cache_files().is_empty());
}

#[test]
fn e2e_ext_one_slow_program_does_not_hold_back_the_others() {
    let lab = Lab::new("parallel");
    let slow = Rig::new("ext-slow");
    *slow.mode.lock().unwrap() = Mode::Silent;
    let fast = Rig::new("ext-fast");
    lab.program("slow-prog", "v1");
    lab.program("fast-prog", "v1");
    let t = tables(&[
        ("ext-slow", table("slow-prog", &[])),
        ("ext-fast", table("fast-prog", &[])),
    ]);
    // One environment, a launcher that sends each program to its rig.
    let mut env = lab.env(&fast);
    env.handshake_limit = Duration::from_millis(400);
    let (slow_f, fast_f) = (slow.factory(), fast.factory());
    env.launcher = Some(std::sync::Arc::new(move |spec, exe| {
        if spec.command == "slow-prog" {
            slow_f(spec, exe)
        } else {
            fast_f(spec, exe)
        }
    }));
    let token = external::install(&env, &t, &[]).unwrap();
    let kept: Vec<String> = external::plugins_of(&token)
        .iter()
        .map(|p| p.manifest().id)
        .collect();
    assert_eq!(kept, vec!["ext-fast".to_string()]);
    assert_eq!(external::problems(&token, crate::i18n::Lang::En).len(), 1);
}

// ---- `--plugin` ----------------------------------------------------------------------------------------

#[test]
fn e2e_ext_a_dev_program_is_registered_under_the_id_of_its_manifest() {
    let lab = Lab::new("dev");
    let rig = Rig::new("ext-dev");
    let exe = lab.program("dev-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let token = external::install(
        &lab.env(&rig),
        &PluginTables::new(),
        std::slice::from_ref(&exe),
    );
    let mut s = session(&lab, token, PluginTables::new());
    assert_eq!(registry_of(&s).origin("ext-dev"), Some(PluginOrigin::User));
    open_file(&mut s, "a.xd");
    s.see("child says hello");
}

#[test]
fn e2e_ext_a_dev_program_that_is_missing_is_reported_and_not_registered() {
    let lab = Lab::new("dev_missing");
    let rig = Rig::new("ext-devm");
    let missing: PathBuf = lab.progs.join("nope");
    let token = external::install(&lab.env(&rig), &PluginTables::new(), &[missing]).unwrap();
    assert!(external::plugins_of(&token).is_empty());
    let p = external::problems(&token, crate::i18n::Lang::En);
    assert!(p[0].contains("--plugin"), "{p:?}");
}

// ---- the listing -------------------------------------------------------------------------------------------

#[test]
fn e2e_ext_the_plugins_listing_shows_the_external_plugins_and_their_commands() {
    let lab = Lab::new("listing");
    let rig = Rig::new("ext-list");
    lab.program("list-prog", "v1");
    let t = tables(&[(
        "ext-list",
        table(
            "list-prog",
            &[(
                "args",
                toml::Value::Array(vec![toml::Value::String("--flag".into())]),
            )],
        ),
    )]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    let cfg = Config {
        plugins: t,
        external_plugins: token,
        ..Config::default()
    };
    let out = crate::plugins::plugins_table(&cfg);
    assert!(out.contains("External plugins"), "{out}");
    assert!(
        out.contains("ext-list") && out.contains("list-prog --flag"),
        "{out}"
    );
    assert!(out.contains("*.xd") && out.contains("external"), "{out}");
    // Without the set, nothing of it is listed.
    let out = crate::plugins::plugins_table(&Config::default());
    assert!(!out.contains("ext-list"));
}

// ---- the read scope of a picture the child names ---------------------------------------------------------------

#[test]
fn e2e_ext_a_picture_file_the_child_names_outside_its_read_scope_is_refused() {
    let lab = Lab::new("scope");
    let rig = Rig::pictures("ext-scope");
    lab.program("scope-prog", "v1");
    let outside = lab.dir.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let outside = std::fs::canonicalize(outside).unwrap();
    png(&outside.join("pic.png"));
    png(&lab.root().join("inside.png"));
    write_xd(
        &lab,
        "out.xd",
        &format!("image:{}", outside.join("pic.png").display()),
    );
    write_xd(
        &lab,
        "in.xd",
        &format!("image:{}", lab.root().join("inside.png").display()),
    );
    let t = tables(&[("ext-scope", table("scope-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    let mut s = session(&lab, token, t).with_picker();

    open_file(&mut s, "out.xd");
    assert!(
        matches!(
            s.app.image_failure(),
            Some(crate::preview::image::ImageFailure::OutsideReadDirs)
        ),
        "{}",
        s.screen()
    );

    s.esc();
    open_file(&mut s, "in.xd");
    s.dont_see("outside what plugin");
}

#[test]
fn e2e_ext_read_dirs_widens_the_scope_of_an_external_plugin() {
    let lab = Lab::new("scope_dirs");
    let rig = Rig::pictures("ext-sd");
    lab.program("sd-prog", "v1");
    let outside = lab.dir.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let outside = std::fs::canonicalize(outside).unwrap();
    png(&outside.join("pic.png"));
    write_xd(
        &lab,
        "out.xd",
        &format!("image:{}", outside.join("pic.png").display()),
    );
    let t = tables(&[(
        "ext-sd",
        table(
            "sd-prog",
            &[(
                "read_dirs",
                toml::Value::Array(vec![toml::Value::String(
                    outside.to_string_lossy().into_owned(),
                )]),
            )],
        ),
    )]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    let mut s = session(&lab, token, t).with_picker();
    open_file(&mut s, "out.xd");
    assert!(
        !matches!(
            s.app.image_failure(),
            Some(crate::preview::image::ImageFailure::OutsideReadDirs)
        ),
        "{}",
        s.screen()
    );
}

// ---- a child that is too slow ---------------------------------------------------------------------------------

#[test]
fn e2e_ext_a_child_that_takes_too_long_shows_cannot_preview_with_a_reason() {
    let lab = Lab::new("slow_open");
    let rig = Rig::new("ext-slowopen");
    lab.program("slowopen-prog", "v1");
    write_xd(&lab, "hang.xd", "hang");
    let t = tables(&[("ext-slowopen", table("slowopen-prog", &[]))]);
    let mut env = lab.env(&rig);
    env.limits.open_view = Duration::from_millis(300);
    env.limits.cancel_grace = Duration::from_millis(100);
    let token = external::install(&env, &t, &[]);
    let mut s = session(&lab, token, t);
    open_file(&mut s, "hang.xd");
    // The reason is on the screen (the host's unavailable view: reason, then the file).
    s.see("the plugin did not answer in time");
    rig.ctl.release.set();
    assert!(rig.kills() >= 1, "the stuck child was killed");
}

// ---- idle children and leaving -----------------------------------------------------------------------------------

#[test]
fn e2e_ext_an_idle_child_is_stopped_by_reap_and_started_again_on_demand() {
    let lab = Lab::new("reap");
    let rig = Rig::new("ext-reap");
    lab.program("reap-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[("ext-reap", table("reap-prog", &[]))]);
    let env = lab.env(&rig);
    let idle_after = env.limits.idle_after;
    let token = external::install(&env, &t, &[]).unwrap();
    let mut s = session(&lab, Some(token.clone()), t);
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    // The file is closed: the session goes with it.
    s.esc();

    // Judged at a moment after the idle time (no waiting): too early does nothing.
    let plugin = external::plugins_of(&token).remove(0);
    assert!(plugin.is_running());
    assert_eq!(external::reap_set_at(&token, Instant::now()), 0);
    assert_eq!(rig.kills(), 0);
    assert!(plugin.is_running());
    let later = Instant::now() + idle_after + Duration::from_secs(1);
    assert_eq!(external::reap_set_at(&token, later), 1);
    assert_eq!(rig.kills(), 1);
    assert!(!plugin.is_running());
    // Nothing more to stop.
    assert_eq!(external::reap_set_at(&token, later), 0);

    // The next open starts a new child.
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    assert_eq!(rig.launches(), 2);
}

#[test]
fn e2e_ext_a_child_with_a_file_open_is_not_reaped() {
    let lab = Lab::new("reap_busy");
    let rig = Rig::new("ext-busy");
    lab.program("busy-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[("ext-busy", table("busy-prog", &[]))]);
    let env = lab.env(&rig);
    let idle_after = env.limits.idle_after;
    let token = external::install(&env, &t, &[]).unwrap();
    let mut s = session(&lab, Some(token.clone()), t);
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    let later = Instant::now() + idle_after + Duration::from_secs(1);
    assert_eq!(
        external::reap_set_at(&token, later),
        0,
        "a live session keeps it"
    );
    assert_eq!(rig.kills(), 0);
}

#[test]
fn e2e_ext_the_run_loop_reaps_at_an_interval_and_never_twice_in_a_row() {
    // Nothing loaded by this test, but another may have: the throttle is what is looked at.
    let lab = Lab::new("reap_throttle");
    let rig = Rig::new("ext-throttle");
    lab.program("throttle-prog", "v1");
    let t = tables(&[("ext-throttle", table("throttle-prog", &[]))]);
    let _token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let tx2 = tx.clone();
    let t0 = Instant::now();
    assert!(external::reap_if_due_with_for_test(t0, move || {
        let _ = tx.send(());
    }));
    rx.recv_timeout(PATIENCE)
        .expect("the work ran on its thread");
    // Again within the interval: not started.
    assert!(!external::reap_if_due_with_for_test(
        t0 + external::REAP_INTERVAL - Duration::from_secs(1),
        move || {
            let _ = tx2.send(());
        }
    ));
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    // After the interval: started again.
    let (tx3, rx3) = std::sync::mpsc::channel::<()>();
    assert!(external::reap_if_due_with_for_test(
        t0 + external::REAP_INTERVAL + Duration::from_secs(1),
        move || {
            let _ = tx3.send(());
        }
    ));
    rx3.recv_timeout(PATIENCE).expect("ran again");
}

#[test]
fn e2e_ext_leaving_stops_the_children_and_starts_no_other() {
    let lab = Lab::new("leave");
    let rig = Rig::new("ext-leave");
    lab.program("leave-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    write_xd(&lab, "b.xd", "plain");
    let t = tables(&[("ext-leave", table("leave-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]).unwrap();
    let mut s = session(&lab, Some(token.clone()), t);
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    assert_eq!(rig.kills(), 0);

    external::shutdown_set(&token);
    assert_eq!(rig.kills(), 1, "the child was stopped");

    // A file opened afterwards is not shown by a new child.
    s.esc();
    open_file(&mut s, "b.xd");
    assert_eq!(rig.launches(), 1, "no child was started while leaving");
    s.see("konoma is leaving");
}

#[test]
fn e2e_ext_the_exit_cleanup_really_stops_the_children() {
    // Process-wide and one-way (the shutting-down flag), so it runs in a process of its own.
    const ENV: &str = "KONOMA_TEST_EXT_EXIT_CHILD";
    if std::env::var_os(ENV).is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "e2e_tests::external::e2e_ext_the_exit_cleanup_really_stops_the_children",
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
    let lab = Lab::new("exit_real");
    let rig = Rig::new("ext-exit");
    lab.program("exit-prog", "v1");
    write_xd(&lab, "a.xd", "plain");
    let t = tables(&[("ext-exit", table("exit-prog", &[]))]);
    let token = external::install(&lab.env(&rig), &t, &[]);
    let mut s = session(&lab, token, t);
    open_file(&mut s, "a.xd");
    s.see("child says hello");
    assert_eq!(rig.kills(), 0);

    crate::preview::command::exit_cleanup();

    assert!(external::is_shutting_down());
    assert_eq!(rig.kills(), 1, "exit_cleanup stopped the child");
}

#[test]
fn e2e_ext_children_work_and_hand_off_under_the_hosts_private_places() {
    // The environment the host builds for real: the working directory and the handoff directory
    // are under the process's private directory, not the folder konoma browses or the cwd.
    let env = Env::production(crate::i18n::Lang::En);
    let private = crate::preview::private_dir::private_dir("probe");
    let parent = private.parent().unwrap().to_path_buf();
    assert!(
        env.work_dir.starts_with(&parent),
        "{:?} {:?}",
        env.work_dir,
        parent
    );
    assert!(
        env.scratch_root.starts_with(&parent),
        "{:?} {:?}",
        env.scratch_root,
        parent
    );
    assert!(env.cache_dir.ends_with("konoma/plugin-manifests"));
}
