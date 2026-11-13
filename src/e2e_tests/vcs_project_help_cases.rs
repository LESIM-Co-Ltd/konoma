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
        "detail" => {
            old.open_git_log();
            old.open_git_commit_detail();
        }
        // The uncommitted-changes detail (the graph's Uncommitted row, `Enter`).
        "worktree_detail" => {
            old.open_git_graph();
            old.open_git_graph_detail();
            assert!(
                old.is_git_detail(),
                "the old graph's Uncommitted detail did not open ({})",
                root.display()
            );
        }
        other => panic!("unknown screen {other}"),
    }
    without_tab_step_row(help_lines_without_actions(
        crate::ops::help::sections(&old),
        lang,
        screen != "changes",
    ))
}

/// Excluded difference (rule: a key that does nothing gets no hint): with one tab open `[` and
/// `]` return at once, so the project canvas's help has no `[ / ]` row (`gate.rs`: they act only
/// with two tabs or more), while the old hub listed it regardless. The exclusion is itself
/// checked: the old help must have exactly one such row.
fn without_tab_step_row(lines: Vec<String>) -> Vec<String> {
    let before = lines.len();
    let kept: Vec<String> = lines
        .into_iter()
        .filter(|l| l.get(3..19).map(str::trim_end) != Some("[ / ]"))
        .collect();
    assert_eq!(
        kept.len() + 1,
        before,
        "the old help no longer has exactly one `[ / ]` row"
    );
    kept
}

/// The screens of one project kind and the fewest help lines (Space menu excluded) each must
/// compare, so the exclusion cannot swallow a whole screen.
fn help_oracle_screens(id: &str) -> Vec<(&'static str, usize)> {
    if id == "git" {
        vec![
            ("changes", 30),
            ("log", 25),
            ("graph", 28),
            ("branches", 22),
            // Compared per row in `vcs_port_worktrees_help_equals_old_hub_per_row`.
            ("worktrees", 0),
            // The pushed commit detail (log, Enter): the old hub's `Screen::Detail`
            // (measured: 26 lines, the Space menu excluded).
            ("detail", 25),
        ]
    } else {
        vec![
            ("changes", 26),
            ("log", 25),
            ("graph", 27),
            ("branches", 19),
            ("detail", 25),
        ]
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
                if screen_name == "detail" {
                    screen(&mut sim, "log");
                    press(&mut sim, KeyCode::Enter);
                    assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
                } else {
                    screen(&mut sim, screen_name);
                }
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

/// Old hub: `src/ops/help.rs` `screen_rows` (`Screen::Detail`, shared by the commit detail and
/// the worktree detail). The detail the graph's Uncommitted row opens with `Enter` (git and jj),
/// and the one the worktrees list opens with `d` (git), show the old hub's help text, en and ja.
#[test]
fn vcs_port_worktree_detail_help_equals_old_hub() {
    use crate::i18n::Lang;
    if isolated("vcs_port_worktree_detail_help_equals_old_hub") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_help_wtdetail_git", true);
    let mut roots = vec![("git", git.to_path_buf())];
    let jj = if crate::vcs::jj::available() {
        crate::test_support::jj_scratch_seeded("vcs_help_wtdetail_jj")
    } else {
        None
    };
    if let Some(jj) = &jj {
        std::fs::write(jj.join("uncommitted.txt"), "new\n").unwrap();
        roots.push(("jj", jj.to_path_buf()));
    }
    let mut compared = 0;
    for (id, root) in &roots {
        for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
            let old = old_hub_help(root, lang, "worktree_detail");
            // Measured: the commit detail's 26 lines (git), the same screen.
            assert!(old.len() >= 20, "{id} {name}: only {} old lines", old.len());
            let mut sim = open(root, config(id, name), (220, 60));
            screen(&mut sim, "graph");
            assert!(
                sim.app.project_frame().unwrap().cursor == 0,
                "the graph opens on its Uncommitted row"
            );
            press(&mut sim, KeyCode::Enter);
            assert_eq!(
                sim.app.project_frame().unwrap().key.screen,
                "detail",
                "{id} {name}: Enter on Uncommitted opens the detail"
            );
            let new = help_lines_without_actions(crate::ops::help::sections(&sim.app), lang, true);
            assert_eq!(
                new,
                old,
                "{id} {name}: worktree detail help differs from the old hub\nNEW:\n{}\nOLD:\n{}",
                new.join("\n"),
                old.join("\n")
            );
            compared += old.len();
        }
    }
    assert!(compared >= 20 * 2 * roots.len(), "only {compared} lines compared");

    // The worktrees list's `d` (git): the old hub opens the same `Screen::Detail`.
    let dir = sandbox("vcs_help_wtdetail_list").into_canonical().unwrap();
    let (root, _linked, _detached) = super::vcs_log::seed_worktrees(&dir);
    // The current worktree's row (the list opens on it) has something to show.
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let mut old = crate::app::App::new(root.to_path_buf(), crate::config::Config::default())
            .unwrap();
        old.lang = lang;
        old.open_git_view();
        old.open_git_worktrees();
        old.worktree_show_changes();
        assert!(old.is_git_detail(), "{name}: the old worktrees `d` did not open a detail");
        // Excluded difference (rule: a key that does nothing gets no hint): the old hub listed a
        // `[Copy commit]` section on this detail, but a worktree's changes have no commit, so the
        // canvas declares no copy act on `worktree_diff` and the new help has no copy section.
        // The exclusion is itself checked: the old section must exist, the new must have none.
        let copy_title = tr(lang, Msg::WkGitCopyTitle);
        let mut old_sections = crate::ops::help::sections(&old);
        let before = old_sections.len();
        old_sections.retain(|s| s.title != copy_title);
        assert_eq!(
            old_sections.len() + 1,
            before,
            "{name}: the old worktree detail help no longer has a [{copy_title}] section"
        );
        let old_lines = without_tab_step_row(help_lines_without_actions(old_sections, lang, true));
        let mut sim = open(&root, config("git", name), (220, 60));
        screen(&mut sim, "worktrees");
        keys(&mut sim, "d");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "worktree_diff", "{name}");
        let new_sections = crate::ops::help::sections(&sim.app);
        for title in [Msg::WkGitCopyTitle, Msg::WkBranchCopyTitle, Msg::WkCopyPathTitle] {
            let title = tr(lang, title);
            assert!(
                new_sections.iter().all(|s| s.title != title),
                "{name}: worktree detail help has a [{title}] section"
            );
        }
        let new = help_lines_without_actions(new_sections, lang, true);
        // Measured: 19 lines once the old `[Copy commit]` section is excluded.
        assert!(old_lines.len() >= 15, "{name}: only {} lines", old_lines.len());
        assert_eq!(
            new,
            old_lines,
            "{name}: worktrees `d` detail help\nNEW:\n{}\nOLD:\n{}",
            new.join("\n"),
            old_lines.join("\n")
        );
    }
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
        // Measured: 21 compared lines with the `Ctrl-t` row, 20 without (no `[ / ]` row).
        assert!(old.len() >= 21,"only {} old lines compared", old.len());
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
    assert!(compared >= 20 * rows * 2,"only {compared} help lines compared");
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
        // `w`) are rebindable too (`goto_<screen>`), but that is pinned by `vcs_jump_keys_*`
        // in vcs_project_keys_cases.rs, so they are not part of this test.
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

/// Old test `changes_hub_help_shows_jj_section_and_hides_git_write_keys` (old hub help text:
/// `src/ops/help.rs` `screen_rows`, labels `src/i18n.rs` `Jj*`), through the real project canvas
/// on every screen, en and ja: the jj changes help shows its six old rows; no jj help (changes,
/// log, graph, bookmarks, detail) names git's write words (stage, unstage, commit, worktree and
/// their ja words); the git changes help names all of them; no git help (changes, log, graph,
/// branches, worktrees, detail) shows a jj label; the git changes and worktrees helps name
/// worktrees.
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
    // The old hub's labels, as literals (`src/i18n.rs`; the old test read them through `tr`).
    // The `Msg` check below ties the literals to the old catalog entries.
    let jj_rows: [(&str, Msg, [&str; 2]); 6] = [
        ("help:jj:changes-label", Msg::JjChangesLabel, ["jj changes (p)", "jj 変更ハブ (p)"]),
        ("help:jj:close-view", Msg::JjCloseView, ["close jj view", "jj ビューを閉じる"]),
        (
            "help:jj:diff-all",
            Msg::JjDiffAll,
            ["diff of everything against @-", "@- との差分をまとめて表示"],
        ),
        ("help:jj:bookmarks", Msg::JjBookmarksRow, ["bookmarks", "ブックマーク"]),
        (
            "help:jj:external-tool",
            Msg::JjExternalTool,
            ["external jj tool (lazyjj)", "外部 jj ツール (lazyjj)"],
        ),
        (
            "help:jj:sync-literal",
            Msg::JjSyncRow,
            ["let jj snapshot the working copy (confirm)", "jj に作業コピーを取り込ませる (確認)"],
        ),
    ];
    let help_text = |sim: &Sim| -> String {
        crate::ops::help::sections(&sim.app)
            .iter()
            .flat_map(|s| {
                std::iter::once(s.title.clone())
                    .chain(s.rows.iter().map(|(k, d)| format!("{k} {d}")))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let git_screens = ["changes", "log", "graph", "branches", "worktrees", "detail"];
    let jj_screens = ["changes", "log", "graph", "branches", "detail"];
    let at = |sim: &mut Sim, name: &str| {
        if name == "detail" {
            screen(sim, "log");
            press(sim, KeyCode::Enter);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
        } else {
            screen(sim, name);
        }
    };
    let (mut jj_checked, mut git_checked) = (0, 0);
    for (lang, name, idx) in [(Lang::En, "en", 0), (Lang::Jp, "ja", 1)] {
        // Git's write words (the old test's four, plus the Space-menu rows' own words).
        let words = [
            tr(lang, Msg::StageHint),
            tr(lang, Msg::UnstageHint),
            tr(lang, Msg::GitCommit),
            tr(lang, Msg::GitWorktreesRow),
        ];
        assert!(words.iter().all(|w| !w.is_empty()));
        let plain: &[&str] = if lang == Lang::En {
            &["stage", "unstage", "commit", "worktree"]
        } else {
            &["ステージ", "アンステージ", "コミット", "ワークツリー"]
        };
        // The literals are the old hub's catalog entries.
        for (id, m, literals) in &jj_rows {
            assert_eq!(tr(lang, *m), literals[idx], "{name} {id}: the old hub's label");
        }
        // jj: every screen names none of git's write words; the changes screen shows the six
        // jj rows (label, close, diff-all, bookmarks, external tool, sync).
        for screen_name in jj_screens {
            let mut sim = open(&jj, config("jj", name), (220, 60));
            at(&mut sim, screen_name);
            let help = help_text(&sim);
            // The changes screen is the old test's: all four words. On the other screens
            // `commit` and `worktree` are words of the lists themselves (old hub: "commit
            // detail", `Copy commit`, "commit diff / worktree diff"), so only the write-only
            // words (stage, unstage) are checked there.
            let changes = screen_name == "changes";
            for word in words.iter().chain(plain) {
                let list_word = ["commit", "worktree", "コミット", "ワークツリー"]
                    .iter()
                    .any(|w| word.contains(w));
                if !changes && list_word {
                    continue;
                }
                assert!(
                    !help.contains(word),
                    "{name} jj {screen_name}: help names {word:?}:\n{help}"
                );
            }
            if changes {
                for (id, _, literals) in &jj_rows {
                    assert!(
                        help.contains(literals[idx]),
                        "{name} {id}: jj changes help lacks {:?}:\n{help}",
                        literals[idx]
                    );
                    jj_checked += 1;
                }
            }
        }
        // git: the write words show on the changes help; no screen shows a jj label; the
        // changes and worktrees helps name worktrees.
        let jj_labels: [&str; 4] = if lang == Lang::En {
            ["jj changes", "close jj view", "lazyjj", "bookmarks"]
        } else {
            ["jj 変更ハブ", "jj ビューを閉じる", "lazyjj", "ブックマーク"]
        };
        for screen_name in git_screens {
            let mut sim = open(&git, config("git", name), (220, 60));
            at(&mut sim, screen_name);
            let help = help_text(&sim);
            for label in jj_labels {
                assert!(
                    !help.contains(label),
                    "{name} git {screen_name}: help shows the jj label {label:?}:\n{help}"
                );
            }
            if screen_name == "changes" {
                for word in words.iter().chain(plain) {
                    assert!(
                        help.contains(word),
                        "{name} git changes: help lacks {word:?}:\n{help}"
                    );
                }
            }
            if screen_name == "changes" || screen_name == "worktrees" {
                assert!(
                    help.contains(tr(lang, Msg::GitWorktreesRow)),
                    "{name} git {screen_name}: help does not name worktrees:\n{help}"
                );
            }
            git_checked += 1;
        }
    }
    assert_eq!(jj_checked, 2 * 6, "every jj row, both languages");
    assert_eq!(git_checked, 2 * 6, "every git screen, both languages");
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

/// A pushed detail's help follows the effective keys: the layout key moves with `[keys.diff]`
/// and is dropped when cleared, the horizontal-scroll row names the moved `Left` key, and
/// the detail lists no row for a key that does not act on it (no filter, refresh or screen
/// cycle: pushed screens ignore `/`, `r` and `Tab`).
#[test]
fn vcs_port_detail_help_follows_effective_keys_and_lists_only_acting_keys() {
    use crate::i18n::Lang;
    if isolated("vcs_port_detail_help_follows_effective_keys_and_lists_only_acting_keys") {
        return;
    }
    let root = super::vcs_log::detail_repo();
    let detail = |entries: &[(&str, &[(&str, &str)])]| -> Sim {
        let mut cfg = config("git", "en");
        for (surface, keys) in entries {
            cfg.keys.surfaces.insert(
                (*surface).into(),
                keys.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
            );
        }
        let mut sim = open(&root, cfg, (180, 20));
        screen(&mut sim, "log");
        press(&mut sim, KeyCode::Enter);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
        sim
    };
    let layout = tr(Lang::En, Msg::GitLayout);
    let row = |sim: &Sim, word: &str| -> Option<String> {
        all_help_rows(sim)
            .into_iter()
            .find(|(_, d)| d == word)
            .map(|(k, _)| k)
    };
    let default = detail(&[]);
    assert_eq!(row(&default, layout).as_deref(), Some("s"));
    assert_eq!(
        row(&default, tr(Lang::En, Msg::GitHScrollEnds)).as_deref(),
        Some("h / l  0 / $")
    );
    // The rows that do not act on a pushed screen are not listed.
    let rows = all_help_rows(&default);
    for word in [
        tr(Lang::En, Msg::GitFilterByName),
        tr(Lang::En, Msg::ListFilterHelp),
        tr(Lang::En, Msg::Refresh),
        tr(Lang::En, Msg::GitScreensHelp),
        tr(Lang::En, Msg::BackToChanges),
    ] {
        assert!(!rows.iter().any(|(_, d)| d == word), "{word}: {rows:?}");
    }
    for key in ["/", "r", "Tab / ⇧Tab"] {
        assert!(!rows.iter().any(|(k, _)| k == key), "{key}: {rows:?}");
    }
    // The keys really do nothing on the detail (the rows' absence is true).
    for key in ['/', 'r'] {
        let mut sim = detail(&[]);
        keys(&mut sim, &key.to_string());
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail", "{key}");
        assert!(sim.app.pending_leader.is_none(), "{key}");
    }
    let mut sim = detail(&[]);
    press(&mut sim, KeyCode::Tab);
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail", "Tab");
    // `h` pans on the detail, it does not leave it (the row says so).
    let mut sim = detail(&[]);
    keys(&mut sim, "h");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail", "h");

    let moved = detail(&[("diff", &[("s", "noop"), ("Z", "cycle_diff_layout")])]);
    assert_eq!(row(&moved, layout).as_deref(), Some("Z"));
    let cleared = detail(&[("diff", &[("s", "noop")])]);
    assert_eq!(row(&cleared, layout), None);
    let panned = detail(&[("text", &[("h", "noop"), ("Z", "navigate:left")])]);
    assert_eq!(
        row(&panned, tr(Lang::En, Msg::GitHScrollEnds)).as_deref(),
        Some("Z / l  0 / $")
    );
    // The back row names `q / Esc`, and the moved back key.
    let back = tr(Lang::En, Msg::GitBack);
    assert_eq!(row(&default, back).as_deref(), Some("q / Esc"));
}

/// Old hub: `src/ops/help.rs` `screen_rows` (changes). An empty git changes list differs from the
/// old hub's help by the decided difference only (no `s`/`u`/`x` rows: the keys do not act on an
/// empty list); every other line is the old hub's.
#[test]
fn vcs_port_empty_changes_help_is_the_old_help_without_the_write_rows() {
    use crate::i18n::Lang;
    if isolated("vcs_port_empty_changes_help_is_the_old_help_without_the_write_rows") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_help_empty_changes", false);
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let old = old_hub_help(&root, lang, "changes");
        // Measured: 26 lines on a clean repository (the changes list is empty; no `[ / ]` row).
        assert!(old.len() >= 26,"only {} old lines compared", old.len());
        let sim = open(&root, config("git", name), (220, 60));
        let new = help_lines_without_actions(crate::ops::help::sections(&sim.app), lang, false);
        // On an empty list the per-row keys do not act: `s`, `u` and `x` and the open key
        // `l / Enter` have no row; `S` and `U` (the all-rows variants) keep theirs, named by the
        // key that is left, with words that describe only that key (a row's words describe
        // exactly the keys on it). Rows are `   <keys, 16 wide> <description>`.
        let row = |keys: &str, rest: &str| format!("   {keys:16} {rest}");
        let (stage_all, unstage_all) = if name == "en" {
            ("stage all", "unstage all")
        } else {
            ("全ステージ", "全アンステージ")
        };
        let mut dropped = 0;
        let kept: Vec<String> = old
            .iter()
            .filter_map(|line| {
                let (Some(keys), Some(_)) = (line.get(3..19), line.get(20..)) else {
                    return Some(line.clone());
                };
                match keys.trim_end() {
                    "s / S" => {
                        dropped += 1;
                        Some(row("S", stage_all))
                    }
                    "u / U" => {
                        dropped += 1;
                        Some(row("U", unstage_all))
                    }
                    "x" | "l / Enter" => {
                        dropped += 1;
                        None
                    }
                    _ => Some(line.clone()),
                }
            })
            .collect();
        assert_eq!(dropped, 4, "{name}: four per-row write/open rows in the old help");
        assert_eq!(new, kept, "{name}\nNEW:\n{}\nOLD:\n{}", new.join("\n"), old.join("\n"));
    }
}
