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

/// An empty changes list: the footer equals the old hub's footer on the same empty list (read
/// live from the old hub), except for hints whose key does nothing on an empty list: git's
/// `s`/`u`/`x` (the decided difference: `s/S` and `u/U` become `S` and `U`, `x` is gone) and
/// `l/Enter:diff` (there is no file row to open; both backends).
#[test]
fn vcs_port_empty_changes_footer_equals_old_hub_except_the_inert_stage_hints() {
    use crate::i18n::Lang;
    if isolated("vcs_port_empty_changes_footer_equals_old_hub_except_the_inert_stage_hints") {
        return;
    }
    let old_footer = |root: &std::path::Path, lang: Lang| -> String {
        let mut app =
            crate::app::App::new(root.to_path_buf(), crate::config::Config::default()).unwrap();
        app.lang = lang;
        app.open_git_view();
        crate::ui::status::footer_spans(&app, 220)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    };
    let clean = super::vcs_changes::git_repo("vcs_footer_oldempty_git", false);
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let old = old_footer(&clean, lang);
        let kept: Vec<String> = old
            .split("  ")
            .filter(|t| !t.starts_with("x:") && !t.starts_with("l/Enter:"))
            .map(|t| t.replacen("s/S:", "S:", 1).replacen("u/U:", "U:", 1))
            .collect();
        assert_eq!(kept.len(), old.split("  ").count() - 2, "{old}");
        assert!(
            kept.iter().any(|t| t.starts_with("S:")) && kept.iter().any(|t| t.starts_with("U:")),
            "{old}"
        );
        let sim = open(&clean, config("git", name), (220, 30));
        assert_eq!(footer_line(&sim), kept.join("  "), "git {name} empty changes");
    }
    let Some(jj) = crate::test_support::jj_scratch_bare("vcs_footer_oldempty_jj") else {
        return;
    };
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let old = old_footer(&jj, lang);
        let kept: Vec<&str> = old.split("  ").filter(|t| !t.starts_with("l/Enter:")).collect();
        assert_eq!(kept.len(), old.split("  ").count() - 1, "{old}");
        let sim = open(&jj, config("jj", name), (220, 30));
        assert_eq!(footer_line(&sim), kept.join("  "), "jj {name} empty changes");
    }
}

/// The worktrees footer shows `C-t:new tab` on a row iff Ctrl-t acts on that row (the current
/// worktree, a missing one and a locked one included): the hint follows the effect.
#[test]
fn vcs_port_worktrees_footer_shows_new_tab_iff_ctrl_t_acts() {
    if isolated("vcs_port_worktrees_footer_shows_new_tab_iff_ctrl_t_acts") {
        return;
    }
    let dir = sandbox("vcs_footer_worktrees_ctrl_t").into_canonical().unwrap();
    let (root, _linked, _detached) = super::vcs_log::seed_worktrees(&dir);
    let rows = crate::git::worktrees(&root).len();
    assert!(rows >= 4, "{rows}");
    let (mut acting, mut inert) = (0, 0);
    for index in 0..rows {
        let mut sim = open(&root, config("git", "en"), (220, 30));
        screen(&mut sim, "worktrees");
        for _ in 0..index {
            keys(&mut sim, "j");
        }
        assert_eq!(sim.app.project_frame().unwrap().cursor, index);
        let shown = footer_line(&sim).contains("C-t:new tab");
        let (tabs, tab_root) = (sim.app.tab_count(), sim.app.tab.root.clone());
        sim.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
        drain(&mut sim);
        let acted = sim.app.tab_count() != tabs || sim.app.tab.root != tab_root;
        assert_eq!(shown, acted, "row {index}: footer {}", footer_line(&sim));
        if acted {
            acting += 1;
        } else {
            inert += 1;
        }
    }
    // Both kinds of rows exist in the fixture, so neither branch can pass vacuously.
    assert!(acting >= 1 && inert >= 1, "acting {acting}, inert {inert}");
}

/// Old hub: `src/ops/footer.rs` `screen()` (the footer plans per screen): every screen of git
/// and jj, en and ja, equals the old text word for word and in order (the literals were read
/// from the old hub's own footer on the same repositories).
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
        ("en", "worktrees", format!("j/k:nav  l/Enter:switch  c:new  d:diff  {tail_en}")),
        ("ja", "worktrees", format!("j/k:移動  l/Enter:切替  c:新規  d:差分  {tail_ja}")),
        ("en", "graph", format!("j/k:nav  l/Enter:detail  s:base  x:base off  b:branches  {tail_en}")),
        ("ja", "graph", format!("j/k:移動  l/Enter:詳細  s:基準  x:基準解除  b:ブランチ  {tail_ja}")),
    ];
    for (lang, name, want) in &cases {
        let mut sim = open(&git, config("git", lang), (220, 30));
        let footer = screen_footer(&mut sim, name);
        assert_eq!(&footer, want, "git {lang} {name}");
        // Old hub `jj_changes_hub_footer_uses_jj_words_never_git_write_words`, git half: the
        // write words and the worktree hint show, none of jj's words do.
        if *name == "changes" && *lang == "en" {
            for word in ["stage", "unstage", "commit", "worktree"] {
                assert!(footer.contains(word), "git footer lacks {word:?}: {footer}");
            }
            for word in ["sync", "bookmarks"] {
                assert!(!footer.contains(word), "git footer shows jj word {word:?}: {footer}");
            }
        }
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
        let footer = screen_footer(&mut sim, name);
        assert_eq!(&footer, want, "jj {lang} {name}");
        // Old hub `jj_changes_hub_footer_uses_jj_words_never_git_write_words` (both languages)
        // and `jj_bookmarks_footer_shows_search_only_hint_not_git_branch_ops`: a read-only
        // backend shows none of git's write words or branch operations.
        if *name == "changes" {
            for word in ["stage", "unstage", "commit", "worktree"] {
                assert!(!footer.contains(word), "jj {lang} footer shows git word {word:?}: {footer}");
            }
        }
        if *name == "branches" && *lang == "en" {
            for word in ["checkout", "new", "delete"] {
                assert!(!footer.contains(word), "jj bookmarks footer shows {word:?}: {footer}");
            }
        }
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

/// `Left` rebound with `[keys]` moves the `h` of `h/q:close` (changes) and `h/q/Esc:back` (log)
/// and the key itself; `Left` cleared drops both. Git and jj.
#[test]
fn vcs_port_left_rebound_moves_and_cleared_drops_the_back_key_and_its_hint() {
    if isolated("vcs_port_left_rebound_moves_and_cleared_drops_the_back_key_and_its_hint") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_footer_left_git", true);
    let jj = if crate::vcs::jj::available() {
        crate::test_support::jj_scratch_seeded("vcs_footer_left_jj")
    } else {
        None
    };
    let mut roots = vec![("git", git.to_path_buf())];
    if let Some(jj) = &jj {
        roots.push(("jj", jj.to_path_buf()));
    }
    let with = |id: &str, entries: &[(&str, &str)]| {
        let mut cfg = config(id, "en");
        cfg.keys.surfaces.insert(
            "text".into(),
            entries.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
        );
        cfg
    };
    for (id, root) in &roots {
        for (name, tail) in [("changes", "{}/q:close"), ("log", "{}/q/Esc:back")] {
            let moved = with(id, &[("h", "noop"), ("Z", "navigate:left")]);
            let mut sim = open(root, moved, (220, 30));
            let footer = screen_footer(&mut sim, name);
            assert!(footer.ends_with(&tail.replace("{}", "Z")), "{id} {name}: {footer}");
            keys(&mut sim, "h");
            assert!(sim.app.project_active(), "{id} {name}: the old h still goes back");
            keys(&mut sim, "Z");
            assert!(!sim.app.project_active(), "{id} {name}: the rebound Left does not go back");

            let cleared = with(id, &[("h", "noop")]);
            let mut sim = open(root, cleared, (220, 30));
            let footer = screen_footer(&mut sim, name);
            assert!(
                footer.ends_with(&tail.replace("{}/", "")),
                "{id} {name}: a cleared Left still shows its token: {footer}"
            );
            keys(&mut sim, "h");
            assert!(sim.app.project_active(), "{id} {name}: a cleared h goes back");
        }
    }
}

/// `[external] git_tool = false`: the changes footer still shows `!:tool` and `!` flashes the
/// disabled message and queues no launch, exactly as the old hub does (the old hub does not
/// gate the hint on the flag; its `launch_git_tool` flashes `ExternalGitToolDisabled`).
#[test]
fn vcs_port_tool_off_keeps_the_hint_and_flashes_like_the_old_hub() {
    use crate::i18n::Lang;
    if isolated("vcs_port_tool_off_keeps_the_hint_and_flashes_like_the_old_hub") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_footer_tool_off", true);
    for (lang, name) in [(Lang::En, "en"), (Lang::Jp, "ja")] {
        let mut off = crate::config::Config::default();
        off.external.git_tool = false;
        let mut old = crate::app::App::new(git.to_path_buf(), off).unwrap();
        old.lang = lang;
        old.open_git_view();
        let old_footer: String = crate::ui::status::footer_spans(&old, 220)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(old_footer.contains("!:"), "old hub hint: {old_footer}");
        old.launch_git_tool();
        assert!(!old.take_launch_git_tool());
        assert!(old.flash.is_some());

        let mut cfg = config("git", name);
        cfg.external.git_tool = false;
        let mut sim = open(&git, cfg, (220, 30));
        screen(&mut sim, "changes");
        assert_eq!(footer_line(&sim), old_footer, "{name}: footer with the tool off");
        press(&mut sim, KeyCode::Char('!'));
        assert!(!sim.app.take_launch_git_tool());
        assert_eq!(sim.app.flash, old.flash, "{name}: the `!` flash");
    }
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
