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
fn vcs_port_probe_old_empty() {
    use crate::i18n::Lang;
    let clean = super::vcs_changes::git_repo("vcs_footer_oldempty_git", false);
    let jj = crate::test_support::jj_scratch_bare("vcs_footer_oldempty_jj");
    for lang in [Lang::En, Lang::Jp] {
        let foot = |app: &crate::app::App| -> String {
            crate::ui::status::footer_spans(app, 220)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        let mut app = crate::app::App::new(clean.to_path_buf(), crate::config::Config::default())
            .unwrap();
        app.lang = lang;
        app.open_git_view();
        eprintln!("OLDEMPTY git {lang:?} changes: {}", foot(&app));
        if let Some(jj) = &jj {
            let mut app =
                crate::app::App::new(jj.to_path_buf(), crate::config::Config::default()).unwrap();
            app.lang = lang;
            app.open_git_view();
            eprintln!("OLDEMPTY jj {lang:?} changes: {}", foot(&app));
        }
    }
}

/// Old hub: `src/ops/footer.rs` `screen()` (the footer plans per screen), measured with the old
/// hub's own footer on the same repositories (`vcs_port_probe_old_hub`): every screen of git
/// and jj, en and ja, equals the old text word for word and in order.
#[test]
fn vcs_port_every_screen_footer_equals_old_hub() {
    if isolated("vcs_port_every_screen_footer_equals_old_hub") {
        return;
    }
    let tail_en = "/:search  Tab/⇧Tab:screens  h/q/Esc:back";
    let tail_ja = "/:検索  Tab/⇧Tab:画面  h/q/Esc:戻る";
    let git = super::vcs_changes::git_repo("vcs_footer_every_git", true);
    crate::test_support::run_git(&git, &["branch", "feature"]);
    let cases: [(&str, &str, String); 10] = [
        ("en", "changes", "s/S:stage(all)  u/U:unstage(all)  x:discard  c:commit  l/Enter:diff  d:diff-all  b:branch  w:worktrees  !:tool  /:search  Tab/⇧Tab:screens  h/q:close".into()),
        ("ja", "changes", "s/S:ステージ(全)  u/U:解除(全)  x:破棄  c:コミット  l/Enter:差分  d:全差分  b:ブランチ  w:ワークツリー  !:ツール  /:検索  Tab/⇧Tab:画面  h/q:閉じる".into()),
        ("en", "log", format!("j/k:nav  l/Enter:detail  {tail_en}")),
        ("ja", "log", format!("j/k:移動  l/Enter:詳細  {tail_ja}")),
        ("en", "branches", format!("j/k:nav  l/Enter:checkout  c:new  d:delete  {tail_en}")),
        ("ja", "branches", format!("j/k:移動  l/Enter:切替  c:新規  d:削除  {tail_ja}")),
        ("en", "worktrees", format!("j/k:nav  l/Enter:switch  c:new  C-t:new tab  d:diff  {tail_en}")),
        ("ja", "worktrees", format!("j/k:移動  l/Enter:切替  c:新規  C-t:別タブ  d:差分  {tail_ja}")),
        ("en", "graph", format!("j/k:nav  l/Enter:detail  s:base  x:base off  b:branches  {tail_en}")),
        ("ja", "graph", format!("j/k:移動  l/Enter:詳細  s:基準  x:基準解除  b:ブランチ  {tail_ja}")),
    ];
    for (lang, name, want) in &cases {
        let mut sim = open(&git, config("git", lang), (220, 30));
        assert_eq!(&screen_footer(&mut sim, name), want, "git {lang} {name}");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = crate::test_support::jj_scratch_seeded("vcs_footer_every_jj").unwrap();
    let cases: [(&str, &str, String); 8] = [
        ("en", "changes", "l/Enter:diff  d:diff-all  b:bookmarks  s:sync  !:tool  /:search  Tab/⇧Tab:screens  h/q:close".into()),
        ("ja", "changes", "l/Enter:差分  d:全差分  b:ブックマーク  s:同期  !:ツール  /:検索  Tab/⇧Tab:画面  h/q:閉じる".into()),
        ("en", "log", format!("j/k:nav  l/Enter:detail  {tail_en}")),
        ("ja", "log", format!("j/k:移動  l/Enter:詳細  {tail_ja}")),
        ("en", "branches", format!("j/k:nav  {tail_en}")),
        ("ja", "branches", format!("j/k:移動  {tail_ja}")),
        ("en", "graph", format!("j/k:nav  l/Enter:detail  a:all revisions  b:bookmarks  {tail_en}")),
        ("ja", "graph", format!("j/k:移動  l/Enter:詳細  a:全リビジョン  b:ブックマーク  {tail_ja}")),
    ];
    for (lang, name, want) in &cases {
        let mut sim = open(&jj, config("jj", lang), (220, 30));
        assert_eq!(&screen_footer(&mut sim, name), want, "jj {lang} {name}");
    }
}

/// A rebound back/search key moves the shown key, an unbound one drops its token, and a
/// rebound `Left` moves the `h` of `h/q:close`.
#[test]
fn vcs_port_footer_follows_rebound_keys() {
    if isolated("vcs_port_footer_follows_rebound_keys") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_footer_rebind", true);
    let with = |entries: &[(&str, &str)]| {
        let mut cfg = config("git", "en");
        cfg.keys.surfaces.insert(
            "git.changes".into(),
            entries.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
        );
        cfg
    };
    let sim = open(&git, with(&[("q", "noop"), ("x", "git_close"), ("/", "noop"), ("f", "list_filter_start")]), (220, 30));
    let footer = footer_line(&sim);
    assert!(footer.contains("f:search") && !footer.contains("/:search"), "{footer}");
    assert!(footer.ends_with("h/x:close"), "{footer}");
    let sim = open(&git, with(&[("q", "noop"), ("/", "noop")]), (220, 30));
    let footer = footer_line(&sim);
    assert!(!footer.contains(":search"), "{footer}");
    assert!(footer.ends_with("Tab/⇧Tab:screens  h:close"), "{footer}");
}

/// An empty git changes list shows no stage/unstage/discard hints.
#[test]
fn vcs_port_empty_git_changes_footer_has_no_stage_hints() {
    if isolated("vcs_port_empty_git_changes_footer_has_no_stage_hints") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_footer_empty_git", false);
    let sim = open(&git, config("git", "en"), (220, 30));
    let footer = footer_line(&sim);
    for hint in ["s/S:", "u/U:", "x:discard"] {
        assert!(!footer.contains(hint), "{hint} on an empty list: {footer}");
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
