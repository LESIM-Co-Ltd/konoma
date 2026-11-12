// The project canvas's footers equal the old hub's.

fn footer_line(sim: &Sim) -> String {
    crate::ui::status::footer_spans(&sim.app, 220)
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// The footer of the project's `screen` (reached by cycling with Tab).
fn screen_footer(sim: &mut Sim, name: &str) -> String {
    screen(sim, name);
    footer_line(sim)
}

#[test]
fn vcs_port_probe_old_hub() {
    use crate::i18n::Lang;
    let git = super::vcs_changes::git_repo("vcs_footer_oldprobe_git", true);
    crate::test_support::run_git(&git, &["branch", "feature"]);
    let jj = if crate::vcs::jj::available() {
        crate::test_support::jj_scratch_seeded("vcs_footer_oldprobe_jj")
    } else {
        None
    };
    for lang in [Lang::En, Lang::Jp] {
        let mut app = crate::app::App::new(git.to_path_buf(), crate::config::Config::default())
            .unwrap();
        app.lang = lang;
        let foot = |app: &crate::app::App| -> String {
            crate::ui::status::footer_spans(app, 220)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        app.open_git_view();
        eprintln!("OLD git {lang:?} changes: {}", foot(&app));
        app.open_git_log();
        eprintln!("OLD git {lang:?} log: {}", foot(&app));
        let mut app = crate::app::App::new(git.to_path_buf(), crate::config::Config::default())
            .unwrap();
        app.lang = lang;
        app.open_git_worktrees();
        eprintln!("OLD git {lang:?} worktrees: {}", foot(&app));
        let mut app = crate::app::App::new(git.to_path_buf(), crate::config::Config::default())
            .unwrap();
        app.lang = lang;
        app.open_git_branches();
        eprintln!("OLD git {lang:?} branches: {}", foot(&app));
        if let Some(jj) = &jj {
            let mut app =
                crate::app::App::new(jj.to_path_buf(), crate::config::Config::default()).unwrap();
            app.lang = lang;
            app.open_git_log();
            eprintln!("OLD jj {lang:?} log: {}", foot(&app));
        }
    }
}

#[test]
fn vcs_port_probe_footers() {
    let git = super::vcs_changes::git_repo("vcs_footer_probe_git", true);
    for lang in ["en", "ja"] {
        let mut sim = open(&git, config("git", lang), (220, 30));
        for name in ["changes", "log", "graph", "branches", "worktrees"] {
            eprintln!("PROBE git {lang} {name}: {}", screen_footer(&mut sim, name));
        }
    }
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = crate::test_support::jj_scratch_seeded("vcs_footer_probe_jj").unwrap();
    for lang in ["en", "ja"] {
        let mut sim = open(&jj, config("jj", lang), (220, 30));
        for name in ["changes", "log", "graph", "branches"] {
            eprintln!("PROBE jj {lang} {name}: {}", screen_footer(&mut sim, name));
        }
    }
}

/// Old hub: `src/ui/status.rs` `jj_graph_footer_shows_jj_hint_not_git_base_pin_hint` (the same
/// literals, now read from the project canvas), and the Japanese wording of the old hub's
/// `Msg` words.
#[test]
fn vcs_port_graph_footer_equals_old_hub() {
    if isolated("vcs_port_graph_footer_equals_old_hub") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_footer_graph_git", true);
    let mut sim = open(&git, config("git", "en"), (220, 30));
    let footer = screen_footer(&mut sim, "graph");
    assert_eq!(
        footer,
        "j/k:nav  l/Enter:detail  s:base  x:base off  b:branches  /:search  Tab/⇧Tab:screens  h/q/Esc:back"
    );
    assert!(
        !footer.contains("all revisions"),
        "git graph footer shows a jj-only word: {footer}"
    );
    let mut sim = open(&git, config("git", "ja"), (220, 30));
    assert_eq!(
        screen_footer(&mut sim, "graph"),
        "j/k:移動  l/Enter:詳細  s:基準  x:基準解除  b:ブランチ  /:検索  Tab/⇧Tab:画面  h/q/Esc:戻る"
    );
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = super::vcs_log::jj_repo(false);
    let mut sim = open(&jj, config("jj", "en"), (220, 30));
    let footer = screen_footer(&mut sim, "graph");
    assert_eq!(
        footer,
        "j/k:nav  l/Enter:detail  a:all revisions  b:bookmarks  /:search  Tab/⇧Tab:screens  h/q/Esc:back"
    );
    assert!(
        !footer.contains("base"),
        "jj graph footer shows a git base-pin word: {footer}"
    );
    let mut sim = open(&jj, config("jj", "ja"), (220, 30));
    assert_eq!(
        screen_footer(&mut sim, "graph"),
        "j/k:移動  l/Enter:詳細  a:全リビジョン  b:ブックマーク  /:検索  Tab/⇧Tab:画面  h/q/Esc:戻る"
    );
}
