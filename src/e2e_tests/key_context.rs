//! The key context (`App::key_context`) follows what is on screen, and `handle_key` looks keys up
//! in it: a binding written for one context fires there and nowhere else. Drives the real
//! `handle_key` with real keys (plugin stage 2 / B1).

use super::*;
use crate::keymap::Context;

/// A config with `table` (a `[keys.<table>]` name) holding `key = action`.
fn cfg_keys(table: &str, key: &str, action: &str) -> Config {
    let mut cfg = cfg_en();
    let mut t = std::collections::HashMap::new();
    t.insert(key.to_string(), action.to_string());
    cfg.keys.surfaces.insert(table.to_string(), t);
    cfg
}

#[test]
fn e2e_key_context_follows_what_is_on_screen() {
    let dir = sandbox("key_context_screens");
    seed_files(&dir);
    write_solid_png(&dir.join("pic.png"), 4, 4, [1, 1, 1]);
    let mut s = // The sort menu has no default key (it moves into the `,` menu); bind it.
    Sim::with_config(&canon(&dir), cfg_keys("tree", "s", "open_sort_menu")).with_picker();
    assert_eq!(s.app.key_context(), Some(Context::Tree));

    // Tree overlays and modes.
    s.key('v');
    assert_eq!(s.app.key_context(), Some(Context::TreeVisual));
    s.esc();
    assert_eq!(s.app.key_context(), Some(Context::Tree));
    s.key('s');
    assert_eq!(s.app.key_context(), Some(Context::Sort));
    s.esc();
    s.key('i');
    assert_eq!(s.app.key_context(), Some(Context::Info));
    s.key('i');
    s.key('T');
    assert_eq!(s.app.key_context(), Some(Context::Tabs));
    s.key('T');
    s.key('\'');
    assert_eq!(s.app.key_context(), Some(Context::Bookmarks));
    s.esc();
    s.key('?');
    assert_eq!(s.app.key_context(), Some(Context::Help));
    s.key('q');
    assert_eq!(s.app.key_context(), Some(Context::Tree));

    // Text input has no context: the keymap is not consulted there.
    s.key('/');
    assert_eq!(s.app.key_context(), None);
    s.esc();

    // Plain text is windowed source: `text`.
    s.select("notes.txt");
    s.enter();
    assert!(s.app.is_windowed());
    assert_eq!(s.app.key_context(), Some(Context::Text));
    s.key('v');
    assert_eq!(s.app.key_context(), Some(Context::TextVisual));
    s.esc();
    s.key('q');

    // Rendered Markdown is a `document`; its raw source (`R`) is windowed text again.
    s.select("readme.md");
    s.enter();
    assert!(!s.app.is_windowed());
    assert_eq!(s.app.key_context(), Some(Context::Document));
    s.key('R');
    assert!(s.app.is_raw_source());
    assert_eq!(s.app.key_context(), Some(Context::Text));
    s.key('R');
    assert_eq!(s.app.key_context(), Some(Context::Document));
    s.key('q');

    // A table, and its cell popup.
    s.select("data.csv");
    s.enter();
    assert!(s.app.is_table_preview());
    assert_eq!(s.app.key_context(), Some(Context::Table));
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::TableCell));
    s.esc();
    s.key('q');

    // An image.
    s.select("pic.png");
    s.enter();
    assert!(s.app.is_image_preview());
    assert_eq!(s.app.key_context(), Some(Context::Image));
}

/// A file that cannot be shown is text (the "can not preview" screen keeps the text keys).
#[test]
fn e2e_key_context_of_an_unsupported_file_is_text() {
    let dir = sandbox("key_context_unsupported");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("blob.bin"), [0u8, 159, 146, 150, 0, 1, 2, 3]).unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("blob.bin");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Text));
}

/// A binding under `[keys.document]` fires in a rendered Markdown view and not in source text;
/// `[keys.text]` the other way round; the old name `preview_text` is both.
#[test]
fn e2e_a_binding_fires_only_in_its_own_context() {
    let open = |cfg: Config, file: &str| -> (Sim, crate::test_support::TmpDir) {
        let dir = sandbox("key_context_binding");
        seed_files(&dir);
        let mut s = Sim::with_config(&canon(&dir), cfg);
        s.select(file);
        s.enter();
        (s, dir)
    };
    // `Z` = open the help overlay.
    for (table, in_doc, in_text) in [
        ("document", true, false),
        ("text", false, true),
        ("preview_text", true, true),
    ] {
        let (mut s, _d) = open(cfg_keys(table, "Z", "toggle_help"), "readme.md");
        assert_eq!(s.app.key_context(), Some(Context::Document));
        s.key('Z');
        assert_eq!(
            s.app.key_context() == Some(Context::Help),
            in_doc,
            "[keys.{table}] in a document"
        );
        let (mut s, _d) = open(cfg_keys(table, "Z", "toggle_help"), "notes.txt");
        assert_eq!(s.app.key_context(), Some(Context::Text));
        s.key('Z');
        assert_eq!(
            s.app.key_context() == Some(Context::Help),
            in_text,
            "[keys.{table}] in source text"
        );
    }
}

#[cfg(feature = "git")]
mod git {
    use super::*;
    use crate::keymap::{Backend, Screen};

    #[test]
    fn e2e_key_context_of_the_version_control_screens_follows_the_backend() {
        let dir = sandbox("key_context_git");
        seed_repo(&dir);
        // The hub's `g` / `l` have no default key; bind them to reach the graph and the log.
        let mut cfg = cfg_en();
        hub_screen_keys(&mut cfg);
        let mut s = Sim::with_config(&canon(&dir), cfg);
        let ctx = |b, sc| Some(Context::Screen(b, sc));
        s.key('p');
        assert_eq!(s.app.key_context(), ctx(Backend::Git, Screen::Changes));
        // The same screen opened for jj is another context.
        s.app.git_vcs = crate::vcs::VcsKind::Jj;
        assert_eq!(s.app.key_context(), ctx(Backend::Jj, Screen::Changes));
        s.app.git_vcs = crate::vcs::VcsKind::Git;

        // Enter opens the file's diff.
        s.enter();
        assert!(s.app.is_git_diff_preview());
        assert_eq!(s.app.key_context(), Some(Context::Diff));
        s.app.git_vcs = crate::vcs::VcsKind::Jj;
        assert_eq!(
            s.app.key_context(),
            Some(Context::Diff),
            "the diff of one file is not split by backend"
        );
        s.app.git_vcs = crate::vcs::VcsKind::Git;
        s.key('q');

        for (key, screen) in [
            ('l', Screen::Log),
            ('g', Screen::Graph),
            ('b', Screen::Branches),
            ('w', Screen::Worktrees),
        ] {
            assert_eq!(s.app.key_context(), ctx(Backend::Git, Screen::Changes));
            s.key(key);
            assert_eq!(s.app.key_context(), ctx(Backend::Git, screen), "{key}");
            s.key('q');
        }
        // Log -> commit detail.
        s.key('l');
        s.enter();
        assert_eq!(s.app.key_context(), ctx(Backend::Git, Screen::Detail));
    }

    /// `[keys."jj.changes"]` fires on the changes screen opened for jj only; the old name
    /// `git_changes` reaches both.
    #[test]
    fn e2e_a_backend_binding_fires_only_for_that_backend() {
        let open = |cfg: Config, jj: bool| -> (Sim, crate::test_support::TmpDir) {
            let dir = sandbox("key_context_backend_binding");
            seed_repo(&dir);
            let mut s = Sim::with_config(&canon(&dir), cfg);
            s.key('p');
            if jj {
                s.app.git_vcs = crate::vcs::VcsKind::Jj;
            }
            (s, dir)
        };
        for (table, on_git, on_jj) in [
            ("jj.changes", false, true),
            ("git.changes", true, false),
            ("git_changes", true, true),
        ] {
            for (jj, expect) in [(false, on_git), (true, on_jj)] {
                let (mut s, _d) = open(cfg_keys(table, "Z", "toggle_help"), jj);
                s.key('Z');
                assert_eq!(
                    s.app.key_context() == Some(Context::Help),
                    expect,
                    "[keys.{table:?}] with jj={jj}"
                );
            }
        }
    }
}

// --- the startup flash -----------------------------------------------------------------------

fn cfg_tables(tables: &[(&str, &[(&str, &str)])]) -> Config {
    let mut cfg = cfg_en();
    for (name, entries) in tables {
        let t = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        cfg.keys.surfaces.insert((*name).to_string(), t);
    }
    cfg
}

fn report(cfg: Config, lang: crate::i18n::Lang) -> Option<String> {
    let dir = sandbox("key_context_report");
    seed_files(&dir);
    let mut app = App::new(canon(&dir), cfg).unwrap();
    app.lang = lang;
    app.keymap_report()
}

#[test]
fn e2e_the_startup_flash_names_an_old_table_and_what_replaces_it() {
    use crate::i18n::Lang;
    // Nothing to say about the defaults or about a table that kept its name.
    assert_eq!(report(cfg_en(), Lang::En), None);
    assert_eq!(
        report(cfg_tables(&[("tree", &[("Z", "refresh")])]), Lang::En),
        None
    );
    // One old name: which, and what to write instead.
    let one = cfg_tables(&[("preview_text", &[("Z", "refresh")])]);
    let en = report(one.clone(), Lang::En).expect("a note");
    assert!(
        en.contains("[keys.preview_text] is an old name")
            && en.contains("[keys.text] + [keys.document]"),
        "{en}"
    );
    assert!(!en.contains("more"), "a single note has no tail: {en}");
    let jp = report(one, Lang::Jp).expect("a note");
    assert!(
        jp.contains("[keys.preview_text] は古い名前です")
            && jp.contains("[keys.text] + [keys.document]"),
        "{jp}"
    );
    // Several: the first one, then how many more.
    let three = cfg_tables(&[
        ("preview_text", &[("Z", "refresh")]),
        ("preview_image", &[("Z", "refresh")]),
        ("preview_table", &[("Z", "refresh")]),
    ]);
    let en = report(three.clone(), Lang::En).unwrap();
    assert!(
        en.contains("[keys.preview_image] is an old name") && en.contains("(+2 more)"),
        "{en}"
    );
    let jp = report(three, Lang::Jp).unwrap();
    assert!(jp.contains("(ほか 2 件)"), "{jp}");
}

#[test]
fn e2e_the_startup_flash_shows_the_first_ignored_setting_and_the_first_conflict() {
    use crate::i18n::Lang;
    let cfg = cfg_tables(&[
        ("nonsense", &[("Z", "refresh")]),
        ("tree", &[("zz", "refresh")]),
    ]);
    let en = report(cfg.clone(), Lang::En).unwrap();
    assert!(en.contains("2 invalid key setting(s) ignored"), "{en}");
    // Tables are read in name order, so the first message is stable.
    assert!(
        en.contains("unknown key surface: nonsense") && en.contains("(+1 more)"),
        "{en}"
    );
    let jp = report(cfg, Lang::Jp).unwrap();
    assert!(
        jp.contains("無効な設定2件を無視") && jp.contains("(ほか 1 件)"),
        "{jp}"
    );

    // A conflict says the key, what was kept, and what was refused where.
    let cfg = cfg_tables(&[("tree", &[("t", "refresh")])]);
    let en = report(cfg.clone(), Lang::En).unwrap();
    assert!(en.contains("1 key conflict(s) (using defaults)"), "{en}");
    assert!(
        en.contains("t: kept tab_new_after, dropped refresh in [keys.tree]"),
        "{en}"
    );
    let jp = report(cfg, Lang::Jp).unwrap();
    assert!(
        jp.contains("キー衝突1件")
            && jp.contains("tab_new_after を維持し refresh in [keys.tree] を破棄"),
        "{jp}"
    );

    // All three kinds together are one line.
    let cfg = cfg_tables(&[
        ("tree", &[("t", "refresh")]),
        ("nonsense", &[("Z", "refresh")]),
        ("preview_text", &[("Z", "refresh")]),
    ]);
    let en = report(cfg, Lang::En).unwrap();
    assert!(!en.contains('\n'));
    assert_eq!(en.matches(" / ").count(), 2, "{en}");
}
