// The old per-screen key tables (`[keys."git.changes"]`, `[keys.git_changes]`, ...) read as old
// names of the git and jj plugins' operations (`plugins/old_screens.rs`).

use crate::plugins::old_screens::{act_screen, Target, ROWS};

/// A key no operation of the plugins has by default and the canvas does not act on.
const OLD_KEY: char = 'Z';

/// Everything one key press visibly did on the canvas.
fn old_effect(sim: &mut Sim) -> String {
    let launched = sim.app.take_launch_git_tool();
    let text = format!(
        "screen={} active={} flash={:?} surface={:?} launched={launched}",
        sim.app
            .project_frame()
            .map(|f| f.key.screen.clone())
            .unwrap_or_default(),
        sim.app.project_active(),
        sim.app.flash,
        sim.app.surface(),
    );
    // Each case has its own temp directory: compare the text without its name.
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("konoma_e2e_") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        rest = &tail[tail.find('/').unwrap_or(tail.len())..];
        out.push_str("<repo>");
    }
    out.push_str(rest);
    out
}

/// Get to `name` (a pushed screen is reached the way a user reaches it).
fn old_reach(sim: &mut Sim, name: &str) {
    match name {
        "graph_picker" => {
            screen(sim, "graph");
            press(sim, KeyCode::Char('b'));
        }
        "detail" => {
            screen(sim, "log");
            press(sim, KeyCode::Enter);
        }
        other => screen(sim, other),
    }
    assert_eq!(frame_screen(sim), name, "reaching {name}");
}

/// A fresh repository of the backend (`None`: jj is not installed).
fn old_repo(id: &str, tag: &str) -> Option<std::path::PathBuf> {
    let name = format!("vcs_old_{id}_{tag}");
    if id == "git" {
        let repo = super::vcs_changes::git_repo(&name, true);
        crate::test_support::run_git(&repo, &["branch", "feature"]);
        // The guard is dropped with the test process's temp dir cleanup; keep the path.
        let path = repo.to_path_buf();
        std::mem::forget(repo);
        Some(path)
    } else {
        let repo = crate::test_support::jj_scratch_seeded(&name)?;
        let path = repo.to_path_buf();
        std::mem::forget(repo);
        Some(path)
    }
}

/// The key an operation has by default and the screen it belongs to (a jump: the Changes screen,
/// which every backend has).
fn old_locate(id: &str, op: &str) -> Option<(String, KeyCode)> {
    use konoma_plugin::host::bind::{bind_jumps, keypress_of};
    let registry = crate::plugins::shared_registry(&[]);
    let manifest = registry.manifest(id)?;
    if let Some(jump) = bind_jumps(id, manifest, None).0.iter().find(|j| j.op == op) {
        return Some(("changes".to_string(), jump.key?.code));
    }
    let act = manifest.actions.iter().find(|a| !a.copy && a.id == op)?;
    let key = konoma_plugin::keys::parse_key(act.key.as_deref()?).ok()?;
    Some((act_screen(act)?.to_string(), keypress_of(key).code))
}

fn old_config(id: &str, lang: &str, table: &str, key: char, action: &str) -> Config {
    let mut cfg = config(id, lang);
    cfg.keys
        .surfaces
        .insert(table.into(), [(key.to_string(), action.to_string())].into());
    cfg
}

fn old_report(sim: &Sim) -> String {
    sim.app.keymap_report().unwrap_or_default()
}

#[test]
fn vcs_old_names_every_mapped_name_reaches_its_operation_and_is_noted() {
    if isolated("vcs_old_names_every_mapped_name_reaches_its_operation_and_is_noted") {
        return;
    }
    let (mut checked, mut moved_something) = (0usize, 0usize);
    for id in ["git", "jj"] {
        for row in ROWS {
            let target = if id == "git" { &row.git } else { &row.jj };
            let Target::Op(candidates) = target else {
                continue;
            };
            for op in *candidates {
                let Some((scr, default_key)) = old_locate(id, op) else {
                    continue;
                };
                let Some(old_repo_a) = old_repo(id, "a") else {
                    continue;
                };
                let Some(old_repo_b) = old_repo(id, "b") else {
                    continue;
                };
                // The operation under its new name, on the same key (the default key can be shared
                // with a sibling operation, so it is not the baseline).
                let _ = default_key;
                let mut base = open(
                    &old_repo_b,
                    old_config(id, "en", id, OLD_KEY, op),
                    (120, 30),
                );
                old_reach(&mut base, &scr);
                let before = old_effect(&mut base);
                press(&mut base, KeyCode::Char(OLD_KEY));
                let want = old_effect(&mut base);
                // The old name, on the old table of the same screen, with another key.
                let table = format!("{id}.{scr}");
                let mut sim = open(
                    &old_repo_a,
                    old_config(id, "en", &table, OLD_KEY, row.old),
                    (120, 30),
                );
                old_reach(&mut sim, &scr);
                let (_, legacy) =
                    App::count_legacy_hub_opens(|| press(&mut sim, KeyCode::Char(OLD_KEY)));
                let got = old_effect(&mut sim);
                assert_eq!(legacy, 0, "{id} {} -> {op}", row.old);
                assert_eq!(
                    got,
                    want,
                    "{id}: `{}` on {table} must act as `{op}` ({})",
                    row.old,
                    old_report(&sim)
                );
                if want != before {
                    moved_something += 1;
                }
                let report = old_report(&sim);
                let new = format!("[keys.{id}] {OLD_KEY} = \"{op}\"");
                assert!(report.contains(&new), "{id} {}: {report}", row.old);
                assert!(
                    report.contains(&format!("{table}\"] {OLD_KEY} = \"{}\"", row.old)),
                    "{id} {}: the note names the old entry: {report}",
                    row.old
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 40, "the loop must cover the table, checked {checked}");
    assert!(
        moved_something >= 20,
        "most operations must change what is on screen, only {moved_something} did"
    );
}

#[test]
fn vcs_old_names_the_note_is_in_both_languages() {
    if isolated("vcs_old_names_the_note_is_in_both_languages") {
        return;
    }
    let repo = old_repo("git", "lang").unwrap();
    for lang in ["en", "ja"] {
        let sim = open(
            &repo,
            old_config("git", lang, "git.changes", OLD_KEY, "git_stage"),
            (120, 30),
        );
        let report = old_report(&sim);
        let head = crate::i18n::tr(
            if lang == "en" {
                crate::i18n::Lang::En
            } else {
                crate::i18n::Lang::Jp
            },
            crate::i18n::Msg::Keymap,
        );
        assert!(report.starts_with(&format!("{head}: ")), "{lang}: {report}");
        assert!(
            report.contains("[keys.git] Z = \"stage\""),
            "{lang}: {report}"
        );
    }
}

#[test]
fn vcs_old_names_an_unmapped_name_does_nothing_and_says_so() {
    if isolated("vcs_old_names_an_unmapped_name_does_nothing_and_says_so") {
        return;
    }
    let mut checked = 0usize;
    for id in ["git", "jj"] {
        for row in ROWS {
            let target = if id == "git" { &row.git } else { &row.jj };
            let Target::None(why) = target else {
                continue;
            };
            let Some(repo) = old_repo(id, "n") else {
                continue;
            };
            // On every screen the backend has: the entry must not act on any of them (the host
            // used to run the old action against the hidden legacy view).
            for scr in ["changes", "log", "graph", "branches"] {
                let mut sim = open(
                    &repo,
                    old_config(id, "en", &format!("{id}.{scr}"), OLD_KEY, row.old),
                    (120, 30),
                );
                screen(&mut sim, scr);
                let before = old_effect(&mut sim);
                let (_, legacy) =
                    App::count_legacy_hub_opens(|| press(&mut sim, KeyCode::Char(OLD_KEY)));
                assert_eq!(legacy, 0);
                assert_eq!(
                    old_effect(&mut sim),
                    before,
                    "{id} {scr}: `{}` has no counterpart, so the key does nothing",
                    row.old
                );
                let report = old_report(&sim);
                assert!(
                    report.contains(&format!("no effect — {why}")),
                    "{id} {scr} {}: {report}",
                    row.old
                );
                assert!(report.contains(row.old), "{report}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 30, "checked {checked}");
}

#[test]
fn vcs_old_names_the_unmapped_note_is_in_ja_too() {
    if isolated("vcs_old_names_the_unmapped_note_is_in_ja_too") {
        return;
    }
    let repo = old_repo("git", "ja").unwrap();
    let sim = open(
        &repo,
        old_config("git", "ja", "git.changes", OLD_KEY, "git_open_log"),
        (120, 30),
    );
    let report = old_report(&sim);
    assert!(report.starts_with("キーマップ"), "{report}");
    assert!(report.contains("git_open_log"), "{report}");
    assert!(report.contains("no effect"), "{report}");
}

#[test]
fn vcs_old_names_host_actions_keep_working() {
    if isolated("vcs_old_names_host_actions_keep_working") {
        return;
    }
    let repo = old_repo("git", "host").unwrap();
    let mut sim = open(
        &repo,
        old_config("git", "en", "git.changes", OLD_KEY, "git_close"),
        (120, 30),
    );
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char(OLD_KEY));
    assert!(!sim.app.project_active(), "git_close still leaves the project");
    let mut sim = open(
        &repo,
        old_config("git", "en", "git.changes", OLD_KEY, "git_launch_tool"),
        (120, 30),
    );
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char(OLD_KEY));
    assert!(
        sim.app.take_launch_git_tool(),
        "git_launch_tool still asks for the tool"
    );
}

#[test]
fn vcs_old_names_noop_removes_only_what_the_screen_has() {
    if isolated("vcs_old_names_noop_removes_only_what_the_screen_has") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = old_repo(id, "noop") else {
            continue;
        };
        // The key of an operation that exists on Changes only.
        let (op, key) = if id == "git" { ("unstage", 'u') } else { ("diff_all", 'd') };
        let mut plain = open(&repo, config(id, "en"), (120, 30));
        screen(&mut plain, "changes");
        let before = old_effect(&mut plain);
        press(&mut plain, KeyCode::Char(key));
        assert_ne!(old_effect(&mut plain), before, "{id}: `{op}` acts by default");

        let mut sim = open(
            &repo,
            old_config(id, "en", &format!("{id}.changes"), key, "noop"),
            (120, 30),
        );
        screen(&mut sim, "changes");
        let before = old_effect(&mut sim);
        press(&mut sim, KeyCode::Char(key));
        assert_eq!(old_effect(&mut sim), before, "{id}: {key} no longer acts");
        let report = old_report(&sim);
        assert!(
            report.contains(&format!("[keys.{id}] {key} = \"noop\"")),
            "{id}: {report}"
        );
    }
}

#[test]
fn vcs_old_names_noop_of_a_shared_key_stays_on_its_own_screen() {
    if isolated("vcs_old_names_noop_of_a_shared_key_stays_on_its_own_screen") {
        return;
    }
    // `s` is `stage` on Changes and `graph_set_base` on Graph. The plugin table cannot take it
    // from one screen only, so the old entry stays a host entry of its own screen.
    let repo = old_repo("git", "noop_s").unwrap();
    let mut default = open(&repo, config("git", "en"), (120, 30));
    screen(&mut default, "graph");
    let g0 = old_effect(&mut default);
    press(&mut default, KeyCode::Char('s'));
    let want_graph = old_effect(&mut default);
    assert_ne!(want_graph, g0, "graph_set_base acts by default");

    let mut sim = open(
        &repo,
        old_config("git", "en", "git.changes", 's', "noop"),
        (120, 30),
    );
    screen(&mut sim, "changes");
    let c0 = old_effect(&mut sim);
    press(&mut sim, KeyCode::Char('s'));
    assert_eq!(old_effect(&mut sim), c0, "s no longer stages on Changes");
    screen(&mut sim, "graph");
    let g1 = old_effect(&mut sim);
    press(&mut sim, KeyCode::Char('s'));
    assert_eq!(old_effect(&mut sim), want_graph, "s still sets the base on Graph (from {g1})");
}

#[test]
fn vcs_old_names_the_new_table_wins() {
    if isolated("vcs_old_names_the_new_table_wins") {
        return;
    }
    let repo = old_repo("git", "wins").unwrap();
    let mut default = open(&repo, config("git", "en"), (120, 30));
    screen(&mut default, "changes");
    let before = old_effect(&mut default);
    press(&mut default, KeyCode::Char('s'));
    let want = old_effect(&mut default);
    assert_ne!(want, before);

    let mut cfg = old_config("git", "en", "git.changes", OLD_KEY, "git_unstage_all");
    cfg.keys
        .surfaces
        .insert("git".into(), [(OLD_KEY.to_string(), "stage".to_string())].into());
    let mut sim = open(&repo, cfg, (120, 30));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char(OLD_KEY));
    assert_eq!(old_effect(&mut sim), want, "[keys.git] decides what Z does");
    let report = old_report(&sim);
    assert!(report.contains("already sets Z"), "{report}");
    assert!(report.contains("git_unstage_all"), "{report}");
}

#[test]
fn vcs_old_names_the_shared_old_table_reaches_both_backends() {
    if isolated("vcs_old_names_the_shared_old_table_reaches_both_backends") {
        return;
    }
    // `[keys.git_changes]` is one name for both backends: `git_worktree_diff` is `diff_all` in each.
    for id in ["git", "jj"] {
        let Some(repo) = old_repo(id, "shared") else {
            continue;
        };
        let mut default = open(&repo, config(id, "en"), (120, 30));
        screen(&mut default, "changes");
        press(&mut default, KeyCode::Char('d'));
        let want = old_effect(&mut default);
        let mut sim = open(
            &repo,
            old_config(id, "en", "git_changes", OLD_KEY, "git_worktree_diff"),
            (120, 30),
        );
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char(OLD_KEY));
        assert_eq!(old_effect(&mut sim), want, "{id}");
        let report = old_report(&sim);
        // Only the first note of the entry is shown (`(+n more)`): the unit tests check each.
        assert!(report.contains("[keys.git_changes] Z = \"git_worktree_diff\""), "{report}");
    }
}
