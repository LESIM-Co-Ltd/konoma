//! Graph comparisons include the unscrolled legend and every body cell.

use super::*;

pub(in crate::e2e_tests) fn fixture() -> TmpDir {
    let dir = repo(1);
    let repository = git2::Repository::open(&dir).unwrap();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    let tree = base.tree().unwrap();
    let make =
        |reference: Option<&str>, subject: &str, parents: &[&git2::Commit<'_>], minute: i64| {
            let signature = git2::Signature::new(
                "作者 Author",
                "fixture@example.invalid",
                &git2::Time::new(1_577_836_800 + minute * 60, 0),
            )
            .unwrap();
            let id = repository
                .commit(reference, &signature, &signature, subject, &tree, parents)
                .unwrap();
            repository.find_commit(id).unwrap()
        };
    let side = make(
        Some("refs/heads/priority-branch-with-a-long-name"),
        "Side 日本語\n\nBody",
        &[&base],
        1,
    );
    let head = make(Some("HEAD"), "Main commit", &[&base], 2);
    let merged = make(Some("HEAD"), "Merge 日本語\n\nBody", &[&head, &side], 3);
    repository
        .branch("second-branch-with-a-long-name", &merged, false)
        .unwrap();
    // Include a staged tracked file, an unstaged edit and an untracked file.
    std::fs::write(dir.join("tracked"), "staged\n").unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(std::path::Path::new("tracked")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.join("tracked"), "unstaged 日本語\n").unwrap();
    std::fs::write(dir.join("untracked"), "untracked\n").unwrap();
    dir
}

pub(in crate::e2e_tests) fn picker_fixture() -> TmpDir {
    let dir = fixture();
    let repository = git2::Repository::open(&dir).unwrap();
    let head = repository.head().unwrap().peel_to_commit().unwrap();
    let base = head.parent(0).unwrap().parent(0).unwrap();
    let tree = base.tree().unwrap();
    let signature = git2::Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(1_577_837_040, 0),
    )
    .unwrap();
    repository
        .commit(
            Some("refs/heads/日本語-branch"),
            &signature,
            &signature,
            "Unmerged 日本語",
            &tree,
            &[&base],
        )
        .unwrap();
    dir
}

/// The project side alone, on the Graph screen (Tab twice from Changes).
fn graph_config(lang: &str, align: &str, cap: usize, priority: bool, icons: bool) -> Config {
    let mut cfg = plugin_config("git", "vcs_graph_parity", lang);
    cfg.ui.commit_meta_align = align.into();
    cfg.ui.graph_max_branches = cap;
    cfg.ui.icons = icons;
    if priority {
        cfg.ui.graph_base_branches = vec![
            "missing-branch".into(),
            "priority-branch-with-a-long-name".into(),
        ];
    }
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert(
        "commit_meta_align".into(),
        toml::Value::String(align.into()),
    );
    settings.insert(
        "graph_max_branches".into(),
        toml::Value::Integer(cap as i64),
    );
    settings.insert(
        "graph_base_branches".into(),
        toml::Value::Array(
            cfg.ui
                .graph_base_branches
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    cfg
}

pub(in crate::e2e_tests) fn graph_sim(
    root: &std::path::Path,
    lang: &str,
    align: &str,
    size: (u16, u16),
    cap: usize,
    priority: bool,
    icons: bool,
) -> Sim {
    let cfg = graph_config(lang, align, cap, priority, icons);
    let mut new = project_sim(root, "git", cfg, size.0, size.1);
    for _ in 0..2 {
        new.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut new);
    }
    assert_eq!(new.app.project_frame().unwrap().key.screen, "graph");
    new
}

/// The picker's top edge (title, `(shown/total)` count, filler, project tabs) is the fixed text
/// from the released appearance at every step of the picker flows: opening, a branch added, every key of the toggle sequence in en and ja
/// with icons on and off, and the jj pickers (not colocated: empty; colocated: git branches).
#[test]
fn vcs_port_picker_title_counts_equal_old_hub_at_every_step() {
    let dir = picker_fixture();
    const GIT_TABS_EN: &str = " p:Changes  Log  Graph  b:Branches  w:Worktrees ";
    const GIT_TABS_JA: &str = " p:変更  ログ  グラフ  b:ブランチ  w:ワークツリー ";
    let git_tabs = |lang: &str| {
        if lang == "en" {
            GIT_TABS_EN
        } else {
            GIT_TABS_JA
        }
    };
    // jj has bookmarks and no worktrees.
    let jj_tabs = |lang: &str| {
        if lang == "en" {
            " p:Changes  Log  Graph  b:Bookmarks "
        } else {
            " p:変更  ログ  グラフ  b:ブックマーク "
        }
    };
    // (shown, total) after opening the panel and after a branch was added, per `icons`. The
    // fixture directory is shared, so the second pass sees the branch added by the first.
    const FIRST: [((usize, usize), (usize, usize)); 2] = [((4, 4), (4, 5)), ((5, 5), (5, 6))];
    // (shown, total) after each key of b x j x a c J K G k g.
    const SEQUENCE: [(usize, usize); 11] = [
        (6, 6),
        (5, 6),
        (5, 6),
        (5, 6),
        (6, 6),
        (2, 6),
        (2, 6),
        (2, 6),
        (2, 6),
        (2, 6),
        (2, 6),
    ];
    for icons in [false, true] {
        let mut new = graph_sim(&dir, "en", "right", (160, 16), 12, false, icons);
        let mut seen = 0;
        new.key('b');
        drain(&mut new);
        picker_edge_is(
            &mut new,
            "en",
            FIRST[usize::from(icons)].0,
            GIT_TABS_EN,
            "open",
        );
        seen += 1;
        new.key('q');
        drain(&mut new);
        let repository = git2::Repository::open(&dir).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        repository
            .branch(&format!("title-added-{icons}"), &head, false)
            .unwrap();
        new.key('b');
        drain(&mut new);
        picker_edge_is(
            &mut new,
            "en",
            FIRST[usize::from(icons)].1,
            GIT_TABS_EN,
            "added",
        );
        seen += 1;
        assert_eq!(seen, 2, "icons={icons}");
    }
    for lang in ["en", "ja"] {
        for icons in [false, true] {
            let mut new = graph_sim(&dir, lang, "right", (160, 16), 12, true, icons);
            let mut seen = 0;
            for key in ['b', 'x', 'j', 'x', 'a', 'c', 'J', 'K', 'G', 'k', 'g'] {
                new.key(key);
                drain(&mut new);
                picker_edge_is(
                    &mut new,
                    lang,
                    SEQUENCE[seen],
                    git_tabs(lang),
                    &format!("{lang} {key}"),
                );
                seen += 1;
            }
            assert_eq!(seen, 11, "{lang} icons={icons}");
        }
    }
    if !crate::vcs::jj::available() {
        return;
    }
    for colocated in [false, true] {
        let jj_dir = crate::test_support::unique_tmp("vcs_picker_title_jj");
        std::fs::create_dir_all(&jj_dir).unwrap();
        let jj_dir = jj_dir.into_canonical().unwrap();
        jj(
            &jj_dir,
            &[
                "git",
                "init",
                if colocated {
                    "--colocate"
                } else {
                    "--no-colocate"
                },
                ".",
            ],
        );
        jj(&jj_dir, &["commit", "-m", "Graph 日本語"]);
        jj(&jj_dir, &["bookmark", "create", "-r", "@-", "日本語"]);
        let _preference = Preference::pin("jj");
        for lang in ["en", "ja"] {
            let cfg = plugin_config("jj", "vcs_picker_title_jj", lang);
            let mut new = project_sim(&jj_dir, "jj", cfg, 160, 16);
            for _ in 0..2 {
                new.press(KeyCode::Tab, KeyModifiers::NONE);
                drain(&mut new);
            }
            new.key('b');
            drain(&mut new);
            // Not colocated: no git branches to pick; colocated: the one git branch.
            let count = if colocated { (1, 1) } else { (0, 0) };
            picker_edge_is(
                &mut new,
                lang,
                count,
                jj_tabs(lang),
                &format!("jj {lang} colocated={colocated}"),
            );
        }
    }
}

/// The picker's top edge equals the old hub's cell for cell (tabs excepted), and the old hub's
/// edge shows the shown and total counts the old hub's own data gives.
/// The picker's top edge as a fixed literal: `┌ <title>  (<shown>/<total>) ──…┐`, every cell
/// unstyled. Pins the released title/count so the test needs no old screen to compare against.
fn picker_edge_is(new: &mut Sim, lang: &str, count: (usize, usize), tabs: &str, what: &str) {
    let title = if lang == "en" {
        "Graph branches"
    } else {
        "グラフのブランチ"
    };
    let width = new.term.backend().buffer().area.width;
    let buffer = new
        .term
        .draw(|frame| ui::render(frame, &mut new.app))
        .unwrap()
        .buffer
        .clone();
    let edge = crate::test_support::top_edge::row_text(&buffer, 1, 0..width);
    let head = format!("┌ {title}  ({}/{}) ", count.0, count.1);
    let rest = edge
        .strip_prefix(&head)
        .unwrap_or_else(|| panic!("{what}: picker edge {edge:?} must start with {head:?}"));
    let rest = rest
        .strip_suffix('┐')
        .unwrap_or_else(|| panic!("{what}: picker edge {edge:?} must end with ┐"));
    // The title, a rule, then the project tabs (their own styling) before the corner.
    let rule = rest.chars().take_while(|c| *c == '─').count();
    assert!(
        rule > 0,
        "{what}: picker edge {edge:?} has a rule after the title"
    );
    let rest_tabs = &rest[rule * '─'.len_utf8()..];
    assert_eq!(rest_tabs, tabs, "{what}: picker edge {edge:?} project tabs");
    let unstyled = u16::try_from(
        head.chars()
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
            .sum::<usize>()
            + rule,
    )
    .unwrap();
    for x in 0..unstyled {
        let cell = &buffer[(x, 1)];
        assert_eq!(
            (cell.fg, cell.bg, cell.modifier),
            (
                ratatui::style::Color::Reset,
                ratatui::style::Color::Reset,
                ratatui::style::Modifier::empty()
            ),
            "{what}: picker edge cell {x} is unstyled"
        );
    }
}

/// The drawn top edge of the frame (row 1), as text.
fn edge_text(sim: &mut Sim) -> String {
    let buf = sim
        .term
        .draw(|frame| ui::render(frame, &mut sim.app))
        .unwrap()
        .buffer
        .clone();
    crate::test_support::top_edge::row_text(&buf, 1, 0..buf.area.width)
}

/// The graph's top edge opens with the label ` Git graph` right after the corner, in both
/// languages (the words stay English), on the canvas's edge.
#[test]
fn vcs_port_git_graph_title_label_is_git_graph_at_the_title_position() {
    use super::vcs_changes::drawn_top_edge;
    let dir = fixture();
    for lang in ["en", "ja"] {
        let mut new = graph_sim(&dir, lang, "right", (160, 16), 12, false, true);
        let edge = drawn_top_edge(&mut new, 160);
        assert!(
            edge.starts_with("┌ Git graph"),
            "lang={lang}: the label sits at the title position: {edge:?}"
        );
        assert!(edge.contains(" Git graph"), "lang={lang}: {edge:?}");
    }
}

/// The graph help (git, both languages) and footer are the old hub's, written out: the graph
/// section's keys and order (`g / G`, `l / Enter`, `s`, `x`, `b`, `/`, `r`, `Tab / ⇧Tab`,
/// `h / Esc  ·  q`), `j / k` under Movement, the Space menu, Tabs and Global sections, and the
/// footer labels. (`[Copy commit]` is absent: the graph opens on its uncommitted row, where
/// `y` copies nothing.)
#[test]
fn vcs_port_graph_help_keys_and_order_equal_legacy() {
    type Section = (&'static str, Vec<(&'static str, &'static str)>);
    let en: Vec<Section> = vec![
        (
            "Git graph",
            vec![
                ("g / G", "top / bottom"),
                ("l / Enter", "detail (commit diff / worktree diff)"),
                (
                    "s",
                    "pin the selected commit's branch as the base (leftmost lane)",
                ),
                ("x", "clear the pinned base"),
                ("b", "choose which branches are shown"),
                ("/", "filter the rows by the text they show"),
                ("r", "refresh listing & git status"),
                ("Tab / ⇧Tab", "cycle changes/log/graph/branches/worktrees"),
                ("h / Esc  ·  q", "back to changes"),
            ],
        ),
        ("Movement", vec![("j / k", "move (commit / uncommitted)")]),
        (
            "Actions",
            vec![
                ("Space s", "base"),
                ("Space x", "base off"),
                ("Space b", "branches"),
                ("Space a", "detail"),
            ],
        ),
        (
            "Tabs",
            vec![
                ("t", "new tab right of this one"),
                ("1 - 9", "jump to tab by number"),
                ("T", "tab list (switch / close)"),
            ],
        ),
        (
            "Global",
            vec![
                (
                    "P",
                    "paste a path / GitHub link and jump there (reveal + preview)",
                ),
                ("?", "toggle this help"),
                ("F", "follow mode: auto-preview externally changed files"),
                ("Q", "quit (back to tree in preview)"),
            ],
        ),
    ];
    let ja: Vec<Section> = vec![
        (
            "Git グラフ",
            vec![
                ("g / G", "先頭 / 末尾"),
                ("l / Enter", "詳細 (コミット差分 / 作業ツリー差分)"),
                ("s", "選択コミットの枝を基準(左端レーン)に固定"),
                ("x", "基準の固定を解除"),
                ("b", "表示するブランチを選ぶ"),
                ("/", "行の表示文字列で絞り込み"),
                ("r", "一覧と git status を再読込"),
                (
                    "Tab / ⇧Tab",
                    "画面を巡回: 変更/ログ/グラフ/ブランチ/worktree",
                ),
                ("h / Esc  ·  q", "変更ハブへ戻る"),
            ],
        ),
        (
            "移動 (Movement)",
            vec![("j / k", "移動 (コミット / 未コミット)")],
        ),
        (
            "操作",
            vec![
                ("Space s", "基準"),
                ("Space x", "基準解除"),
                ("Space b", "ブランチ"),
                ("Space a", "詳細"),
            ],
        ),
        (
            "タブ (Tabs)",
            vec![
                ("t", "右隣に新規タブ"),
                ("1 - 9", "番号でタブへジャンプ"),
                ("T", "タブ一覧(切替 / 閉じる)"),
            ],
        ),
        (
            "共通 (Global)",
            vec![
                (
                    "P",
                    "パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)",
                ),
                ("?", "このヘルプの開閉"),
                ("F", "追尾モード: 外部で変更されたファイルを自動プレビュー"),
                ("Q", "終了 (プレビュー中はツリーへ戻る)"),
            ],
        ),
    ];
    let footer_en = [
        "j/k:nav",
        "l/Enter:detail",
        "s:base",
        "x:base off",
        "b:branches",
        "/:search",
        "Tab/⇧Tab:screens",
        "h/q/Esc:back",
    ];
    let footer_ja = [
        "j/k:移動",
        "l/Enter:詳細",
        "s:基準",
        "x:基準解除",
        "b:ブランチ",
        "/:検索",
        "Tab/⇧Tab:画面",
        "h/q/Esc:戻る",
    ];
    let dir = fixture();
    for (lang, want, footer) in [
        ("en", en, footer_en.as_slice()),
        ("ja", ja, footer_ja.as_slice()),
    ] {
        let new = graph_sim(&dir, lang, "right", (160, 16), 12, false, true);
        let sections = crate::ops::help::sections_for(&new.app, crate::keymap::Context::Canvas);
        let got: Vec<(String, Vec<(String, String)>)> = sections
            .iter()
            .map(|s| (s.title.clone(), s.rows.clone()))
            .collect();
        let want: Vec<(String, Vec<(String, String)>)> = want
            .into_iter()
            .map(|(t, rows)| {
                (
                    t.to_string(),
                    rows.into_iter()
                        .map(|(k, d)| (k.to_string(), d.to_string()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(got, want, "{lang}: the graph help");
        let got_footer = crate::ops::footer::tokens(&new.app, crate::keymap::Context::Canvas);
        assert_eq!(got_footer, footer, "{lang}: the graph footer");
    }
}

/// The jj graph's top edge written out in both languages: the title and commit count, no base
/// text (jj has no base pin), the filler and the project tabs.
#[test]
fn e2e_vcs_graph_jj_top_edge_literal_in_both_languages() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = jj_graph_fixture();
    let _preference = Preference::pin("jj");
    for lang in ["en", "ja"] {
        let cfg = plugin_config("jj", "vcs_graph_jj_edge", lang);
        let mut sim = Sim::with_config_sized(&dir, cfg, 160, 16);
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key('p');
        drain(&mut sim);
        for _ in 0..2 {
            sim.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut sim);
        }
        let edge = edge_text(&mut sim);
        assert!(edge.starts_with("┌ jj graph  ("), "{lang}: {edge:?}");
        assert!(!edge.contains("base:"), "{lang}: {edge:?}");
        assert!(edge.starts_with("┌ jj graph  (2) ─"), "{lang}: {edge:?}");
        let tabs = if lang == "en" {
            "─ p:Changes  Log  Graph  b:Bookmarks ┐"
        } else {
            "─ p:変更  ログ  グラフ  b:ブックマーク ┐"
        };
        assert!(edge.ends_with(tabs), "{lang}: {edge:?}");
    }
}

pub(super) fn jj_graph_fixture() -> TmpDir {
    let dir = crate::test_support::unique_tmp("vcs_graph_jj");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.into_canonical().unwrap();
    jj(&dir, &["git", "init", "--no-colocate", "."]);
    // A local remote pins trunk so the default revset is narrower than all().
    let remote = crate::test_support::unique_tmp("vcs_graph_jj_remote");
    git2::Repository::init_bare(&remote).unwrap();
    jj(
        &dir,
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    for message in ["c1", "c2", "Trunk 日本語"] {
        jj(&dir, &["commit", "-m", message]);
    }
    jj(&dir, &["bookmark", "create", "-r", "@-", "main"]);
    jj(
        &dir,
        &["git", "push", "--remote", "origin", "--bookmark", "main"],
    );
    jj(&dir, &["describe", "-m", "Next 日本語"]);
    dir
}

/// The graph's branch picker for `check_tab_keys_listed_and_act`: the picker is open, with a
/// second tab beside it when `two` (the second tab is made before the picker opens, since
/// switching tabs closes a modal).
fn picker_screen(new: &mut Sim, two: bool, open_graph: impl Fn(&mut Sim)) {
    open_graph(new);
    if two {
        new.app.tab_new().unwrap();
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.app.tab_goto(0);
        drain(new);
    }
    new.key('b');
    drain(new);
    assert_eq!(new.app.project_frame().unwrap().key.screen, "graph_picker");
}

#[test]
fn e2e_vcs_graph_picker_tab_keys_are_listed_where_they_act_git() {
    let dir = picker_fixture();
    super::project_screens::check_tab_keys_listed_and_act("git picker", false, &|two| {
        let mut new = graph_sim(&dir, "en", "right", (160, 40), 12, false, false);
        picker_screen(&mut new, two, |_| {});
        (new, Box::new(()))
    });
}

#[test]
fn e2e_vcs_graph_picker_tab_keys_are_listed_where_they_act_jj() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = crate::test_support::unique_tmp("vcs_picker_tab_keys_jj");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.into_canonical().unwrap();
    jj(&dir, &["git", "init", "--colocate", "."]);
    jj(&dir, &["commit", "-m", "Graph 日本語"]);
    jj(&dir, &["bookmark", "create", "-r", "@-", "日本語"]);
    let _preference = Preference::pin("jj");
    super::project_screens::check_tab_keys_listed_and_act("jj picker", false, &|two| {
        let cfg = plugin_config("jj", "vcs_picker_tab_keys_jj", "en");
        let mut new = project_sim(&dir, "jj", cfg, 160, 16);
        picker_screen(&mut new, two, |new| {
            for _ in 0..2 {
                new.press(KeyCode::Tab, KeyModifiers::NONE);
                drain(new);
            }
        });
        (new, Box::new(()))
    });
}

#[test]
fn e2e_vcs_graph_picker_help_lists_the_global_rows_and_the_keys_act() {
    // The branch picker is a modal list, but `?`, `F`, `Q` (and `P`) act there as on every project
    // screen, so its help lists the Global section, and the Tabs section as well (its keys are
    // pressed by the `..._picker_tab_keys_are_listed_where_they_act_*` tests); `Ctrl-t` does not
    // act there (no tab opens) and is listed nowhere.
    let dir = picker_fixture();
    let rows_of = |new: &Sim, title: &str| -> Vec<(String, String)> {
        crate::ops::help::sections(&new.app)
            .into_iter()
            .filter(|s| s.title == title)
            .flat_map(|s| s.rows)
            .collect()
    };
    for lang in ["en", "ja"] {
        let mut new = graph_sim(&dir, lang, "right", (160, 40), 12, false, false);
        new.key('b');
        drain(&mut new);
        assert_eq!(new.app.project_frame().unwrap().key.screen, "graph_picker");
        let titles: Vec<_> = crate::ops::help::sections(&new.app)
            .into_iter()
            .map(|s| s.title)
            .collect();
        let global = crate::i18n::tr(new.app.lang, crate::i18n::Msg::HelpGlobal);
        let tabs_title = crate::i18n::tr(new.app.lang, crate::i18n::Msg::HelpTabs);
        assert_eq!(
            titles.len(),
            3,
            "{lang}: the picker's own section, Tabs and Global: {titles:?}"
        );
        assert_eq!(titles[1], tabs_title, "{lang}");
        assert_eq!(titles[2], global, "{lang}");
        let keys: Vec<_> = rows_of(&new, global).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["P", "?", "F", "Q"], "{lang}");
        // The keys listed act on the picker.
        let tabs = new.app.tab_count();
        new.key('F');
        assert!(new.app.follow_enabled(), "{lang}: F follows on the picker");
        new.ctrl('t');
        assert_eq!(
            new.app.tab_count(),
            tabs,
            "{lang}: Ctrl-t opens nothing on the picker"
        );
        new.key('?');
        assert!(new.app.show_help, "{lang}: ? opens the help on the picker");
        new.key('?');
        assert!(!new.app.show_help, "{lang}: ? closes it");
        new.key('Q');
        assert!(!new.quit, "{lang}: Q asks first");
        new.key('y');
        assert!(new.quit, "{lang}: Q then y quits from the picker");
    }
}
