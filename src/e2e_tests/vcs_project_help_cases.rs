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
            ("changes", 18),
            ("log", 8),
            ("graph", 12),
            ("branches", 11),
            ("worktrees", 12),
        ]
    } else {
        vec![("changes", 14), ("log", 8), ("graph", 12), ("branches", 8)]
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
        roots.push(("jj", jj.to_path_buf()));
    }
    let mut compared = 0;
    for (id, root) in &roots {
        for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
            let mut sim = open(root, config(id, name), (220, 60));
            for (screen_name, least) in help_oracle_screens(id) {
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
    assert!(compared >= 300, "only {compared} help lines compared");
}
