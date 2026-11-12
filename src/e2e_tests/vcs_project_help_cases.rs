// The project canvas's `?` help equals the old hub's, screen by screen.

/// The help lines of `sections`, one `[title]` line then one line per row, without the Space
/// menu section (its rows belong to the shared-operations unit, not to this comparison).
///
/// `copy_path` is false on the changes screen: the old hub listed a `[Copy path]` menu there, but
/// the canvas's `y` opens nothing on that screen (no copy action is declared; pinned by
/// `vcs_port_copy_empty_and_unbound_prefix_have_no_hints_or_effects`), and a key that does not
/// act has no help row. That one section is compared as absent on both sides.
fn help_lines_without_actions(
    sections: Vec<crate::ui::help::HelpSection>,
    lang: crate::i18n::Lang,
    copy_path: bool,
) -> Vec<String> {
    let menu_title = tr(lang, Msg::WkActionsTitle);
    let copy_path_title = tr(lang, Msg::WkCopyPathTitle);
    let mut lines = Vec::new();
    for section in sections {
        if section.title == menu_title || (!copy_path && section.title == copy_path_title) {
            continue;
        }
        lines.push(format!("[{}]", section.title));
        for (keys, description) in section.rows {
            lines.push(format!("   {keys:16} {description}"));
        }
    }
    lines
}

/// What the old hub's `?` help says on `screen`, read live from the old hub in this tree.
fn old_hub_help(
    root: &std::path::Path,
    lang: crate::i18n::Lang,
    screen: &str,
) -> Vec<String> {
    let mut old = crate::app::App::new(root.to_path_buf(), crate::config::Config::default())
        .unwrap();
    old.lang = lang;
    old.open_git_view();
    match screen {
        "changes" => {}
        "log" => old.open_git_log(),
        "graph" => old.open_git_graph(),
        "branches" => old.open_git_branches(),
        "worktrees" => old.open_git_worktrees(),
        other => panic!("unknown screen {other}"),
    }
    help_lines_without_actions(crate::ops::help::sections(&old), lang, screen != "changes")
}

/// The screens of one project kind and the fewest help lines (Space menu excluded) each must
/// compare, so the exclusion cannot swallow a whole screen.
fn help_oracle_screens(id: &str) -> Vec<(&'static str, usize)> {
    if id == "git" {
        vec![
            ("changes", 31),
            ("log", 26),
            ("graph", 29),
            ("branches", 23),
            // Compared per row in `vcs_port_worktrees_help_equals_old_hub_per_row`.
            ("worktrees", 0),
        ]
    } else {
        vec![("changes", 27), ("log", 26), ("graph", 28), ("branches", 20)]
    }
}

/// Old hub: `src/ops/help.rs` `screen_rows` (every VCS screen) rendered by `sections`: the
/// canvas help of every screen of git and jj, en and ja, equals the old hub's help text line
/// for line (section titles, rows, words, order), except the Space menu section.
#[test]
fn vcs_port_help_equals_old_hub_on_every_screen() {
    use crate::i18n::Lang;
    if isolated("vcs_port_help_equals_old_hub_on_every_screen") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_help_oracle_git", true);
    crate::test_support::run_git(&git, &["branch", "feature"]);
    let mut roots = vec![("git", git.to_path_buf())];
    let jj = if crate::vcs::jj::available() {
        crate::test_support::jj_scratch_seeded("vcs_help_oracle_jj")
    } else {
        None
    };
    if let Some(jj) = &jj {
        // The old hub opens its bookmark list only when a bookmark exists.
        assert!(crate::test_support::run_jj(
            jj,
            &["bookmark", "create", "feature", "-r", "@-"]
        ));
        roots.push(("jj", jj.to_path_buf()));
    }
    let mut compared = 0;
    for (id, root) in &roots {
        for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
            let mut sim = open(root, config(id, name), (220, 60));
            for (screen_name, least) in help_oracle_screens(id) {
                if screen_name == "worktrees" {
                    // Compared per row in `vcs_port_worktrees_help_equals_old_hub_per_row`.
                    continue;
                }
                let old = old_hub_help(root, lang, screen_name);
                screen(&mut sim, screen_name);
                if screen_name == "graph" {
                    // The graph opens on its uncommitted row, where no commit can be copied;
                    // the old hub listed its `y` menu there regardless. Compare on a commit row.
                    let copy = tr(lang, Msg::WkGitCopyTitle);
                    for _ in 0..8 {
                        let on_commit = crate::ops::help::sections(&sim.app)
                            .iter()
                            .any(|s| s.title == copy);
                        if on_commit {
                            break;
                        }
                        keys(&mut sim, "j");
                    }
                }
                let new = help_lines_without_actions(
                    crate::ops::help::sections(&sim.app),
                    lang,
                    screen_name != "changes",
                );
                assert!(
                    old.len() >= least,
                    "{id} {name} {screen_name}: only {} old lines compared",
                    old.len()
                );
                assert_eq!(
                    new,
                    old,
                    "{id} {name} {screen_name}: canvas help differs from the old hub\nNEW:\n{}\nOLD:\n{}",
                    new.join("\n"),
                    old.join("\n")
                );
                compared += old.len();
            }
        }
    }
    let measured: usize = help_oracle_screens("git").iter().chain(&help_oracle_screens("jj")).map(|(_, n)| n).sum();
    assert!(
        compared >= 2 * measured,
        "only {compared} help lines compared, expected at least {}",
        2 * measured
    );
}

/// Old hub: `src/ops/help.rs` `screen_rows` (worktrees) rendered by `sections`. The canvas's
/// worktrees help equals the old hub's on every row, except the decided difference (U9b1t-2a):
/// `Ctrl-t` is named (footer and help alike, one condition `row_target`) only on a row where it
/// acts; on the current worktree's row it does not, so no `Ctrl-t` row is shown.
#[test]
fn vcs_port_worktrees_help_equals_old_hub_per_row() {
    use crate::i18n::Lang;
    if isolated("vcs_port_worktrees_help_equals_old_hub_per_row") {
        return;
    }
    let dir = sandbox("vcs_help_worktrees_rows").into_canonical().unwrap();
    let (root, _linked, _detached) = super::vcs_log::seed_worktrees(&dir);
    let rows = crate::git::worktrees(&root).len();
    assert!(rows >= 4, "{rows}");
    let is_ctrl_t = |line: &String| line.starts_with("   Ctrl-t ");
    let (mut acting, mut inert, mut compared) = (0, 0, 0);
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let old = old_hub_help(&root, lang, "worktrees");
        assert_eq!(old.iter().filter(|l| is_ctrl_t(l)).count(), 1, "{old:?}");
        // Measured: 22 compared lines with the `Ctrl-t` row, 21 without.
        assert!(old.len() >= 22, "only {} old lines compared", old.len());
        let old_without: Vec<String> = old.iter().filter(|l| !is_ctrl_t(l)).cloned().collect();
        for index in 0..rows {
            let mut sim = open(&root, config("git", name), (220, 60));
            screen(&mut sim, "worktrees");
            for _ in 0..index {
                keys(&mut sim, "j");
            }
            assert_eq!(sim.app.project_frame().unwrap().cursor, index);
            let footer_has = footer_line(&sim).contains("C-t:");
            let new = help_lines_without_actions(crate::ops::help::sections(&sim.app), lang, true);
            let help_has = new.iter().any(is_ctrl_t);
            assert_eq!(footer_has, help_has, "{name} row {index}: help follows the footer");
            let want = if help_has { &old } else { &old_without };
            assert_eq!(&new, want, "{name} row {index}\nNEW:\n{}\nOLD:\n{}", new.join("\n"), want.join("\n"));
            compared += new.len();
            if name == "en" {
                if help_has {
                    acting += 1;
                } else {
                    inert += 1;
                }
            }
        }
    }
    assert!(acting >= 1 && inert >= 1, "acting {acting}, inert {inert}");
    assert!(compared >= 21 * rows * 2, "only {compared} help lines compared");
}

/// Every help row of the canvas as (keys, description), Space menu included.
fn all_help_rows(sim: &Sim) -> Vec<(String, String)> {
    crate::ops::help::sections(&sim.app)
        .into_iter()
        .flat_map(|s| s.rows)
        .collect()
}

/// A rebound key moves the help row's key and a cleared key drops the row (git and jj changes
/// screens, the `d` diff-all row and the `b` bookmarks/branches row): the help is built from the
/// effective keys, as the footer is.
#[test]
fn vcs_port_help_follows_rebound_and_cleared_keys() {
    use crate::i18n::Lang;
    if isolated("vcs_port_help_follows_rebound_and_cleared_keys") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_help_rebind_git", true);
    let mut roots = vec![("git", git.to_path_buf())];
    let jj = crate::test_support::jj_scratch_seeded("vcs_help_rebind_jj");
    if let Some(jj) = &jj {
        roots.push(("jj", jj.to_path_buf()));
    }
    for (id, root) in &roots {
        let sim = open(root, config(id, "en"), (220, 60));
        let default = all_help_rows(&sim);
        let diff_all = tr(Lang::En, if *id == "jj" { Msg::JjDiffAll } else { Msg::GitDiffAll });
        // The diff-all key is the plugin's own (`[keys.<plugin>]`). The screen jump keys (`b`,
        // `w`) are the project's declared keys, not rebindable, so they are not part of this test.
        {
            let (word, key, action, surface) = (diff_all, "d", "diff_all", *id);
            let with = |entries: &[(&str, &str)]| {
                let mut cfg = config(id, "en");
                cfg.keys.surfaces.insert(
                    surface.into(),
                    entries.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
                );
                cfg
            };
            let at = default.iter().position(|(k, d)| k == key && d == word);
            assert!(at.is_some(), "{id}: default help has `{key}` {word}: {default:?}");
            let at = at.unwrap();

            let moved = open(root, with(&[(key, "noop"), ("Z", action)]), (220, 60));
            let rows = all_help_rows(&moved);
            // The row keeps its place and its word; only the key moved.
            assert_eq!(rows[at], ("Z".to_string(), word.to_string()), "{id} {key}: {rows:?}");
            assert!(!rows.iter().any(|(k, d)| k == key && d == word), "{id} {key}");

            let cleared = open(root, with(&[(key, "noop")]), (220, 60));
            let rows = all_help_rows(&cleared);
            assert!(
                !rows.iter().any(|(_, d)| d == word),
                "{id} {key}: a cleared key keeps its row: {rows:?}"
            );
            assert!(rows.len() < default.len(), "{id} {key}: the row is gone");
        }
        // The back row names the effective Left key: moved with `[keys.text]`, then cleared.
        let leave = tr(Lang::En, if *id == "jj" { Msg::JjCloseView } else { Msg::GitCloseView });
        let leave_keys = |entries: &[(&str, &str)]| -> String {
            let mut cfg = config(id, "en");
            cfg.keys.surfaces.insert(
                "text".into(),
                entries.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
            );
            let sim = open(root, cfg, (220, 60));
            let rows = all_help_rows(&sim);
            let found: Vec<_> = rows.iter().filter(|(_, d)| d == leave).collect();
            assert_eq!(found.len(), 1, "{id}: the back row: {rows:?}");
            found[0].0.clone()
        };
        assert_eq!(leave_keys(&[]), "h / Esc  ·  q", "{id}");
        assert_eq!(leave_keys(&[("h", "noop"), ("Z", "navigate:left")]), "Z / Esc  ·  q", "{id}");
        assert_eq!(leave_keys(&[("h", "noop")]), "q / Esc", "{id}");
    }
}

/// Old test `changes_hub_help_shows_jj_section_and_hides_git_write_keys`, on every screen kind
/// that has a changes list: the jj changes help (the whole `?` text, Space menu included) names
/// none of git's write words (stage, unstage, commit, worktree and their ja words) and the git
/// changes help names all of them; en and ja.
#[test]
fn vcs_port_jj_help_has_no_git_write_words_and_git_help_has_them() {
    use crate::i18n::Lang;
    if isolated("vcs_port_jj_help_has_no_git_write_words_and_git_help_has_them") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_help_words_git", true);
    let Some(jj) = crate::test_support::jj_scratch_seeded("vcs_help_words_jj") else {
        return;
    };
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let words = [
            tr(lang, Msg::StageHint),
            tr(lang, Msg::UnstageHint),
            tr(lang, Msg::GitCommit),
            tr(lang, Msg::GitWorktreesRow),
        ];
        assert!(words.iter().all(|w| !w.is_empty()));
        let text = |root: &std::path::Path, id: &str| -> String {
            let sim = open(root, config(id, name), (220, 60));
            crate::ops::help::sections(&sim.app)
                .iter()
                .flat_map(|s| {
                    std::iter::once(s.title.clone())
                        .chain(s.rows.iter().map(|(k, d)| format!("{k} {d}")))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let jj_help = text(&jj, "jj");
        let git_help = text(&git, "git");
        for word in words {
            assert!(!jj_help.contains(word), "{name}: jj help names {word:?}:\n{jj_help}");
            assert!(git_help.contains(word), "{name}: git help lacks {word:?}:\n{git_help}");
        }
        if lang == Lang::En {
            for word in ["stage", "unstage", "commit", "worktree"] {
                assert!(!jj_help.contains(word), "jj help names {word:?}:\n{jj_help}");
                assert!(git_help.contains(word), "git help lacks {word:?}:\n{git_help}");
            }
        }
        for m in [
            Msg::JjChangesLabel,
            Msg::JjCloseView,
            Msg::JjDiffAll,
            Msg::JjBookmarksRow,
            Msg::JjExternalTool,
            Msg::JjSyncRow,
        ] {
            assert!(jj_help.contains(tr(lang, m)), "{name}: jj help lacks {m:?}:\n{jj_help}");
        }
    }
}

/// Old hub: `src/ops/help.rs` `screen_rows` (changes: `h / Esc  ·  q` closes the view). `Esc` on
/// the home changes screen leaves the project like the old hub's `Esc` closes the old hub, on git
/// and jj alike (the host's canvas handling, `input.rs` `KeyCode::Esc`).
#[test]
fn vcs_port_esc_on_home_closes_the_view_like_the_old_hub() {
    if isolated("vcs_port_esc_on_home_closes_the_view_like_the_old_hub") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_esc_home_git", true);
    let mut roots = vec![("git", git.to_path_buf())];
    let jj = crate::test_support::jj_scratch_seeded("vcs_esc_home_jj");
    if let Some(jj) = &jj {
        roots.push(("jj", jj.to_path_buf()));
    }
    for (id, root) in &roots {
        let mut old = Sim::with_config_sized(root, crate::config::Config::default(), 100, 20);
        old.app.open_git_view();
        assert!(old.app.is_git_view(), "{id}: old hub opens");
        press(&mut old, KeyCode::Esc);
        assert!(!old.app.is_git_view(), "{id}: Esc closes the old hub");
        let mut new = open(root, config(id, "en"), (100, 20));
        assert_eq!(new.app.project_frame().unwrap().key.screen, "changes");
        press(&mut new, KeyCode::Esc);
        assert!(!new.app.project_active(), "{id}: Esc leaves the project");
    }
}
