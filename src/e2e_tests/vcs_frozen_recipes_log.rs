//! Recipes of the old log and detail tests in `vcs_log.rs`.

use super::super::super::vcs_changes::plugin_config;
use super::super::super::vcs_log::{detail_repo, jj_repo, repo, seed_jj_ids};
use super::super::super::*;
use super::super::Ctx;
use konoma_vcs_core::git2;

/// Pins the backend preference for the current thread until dropped.
struct Pin;
impl Pin {
    fn new(id: &str) -> Self {
        crate::vcs::set_preference_for_test(Some(if id == "git" {
            crate::vcs::Preference::Git
        } else {
            crate::vcs::Preference::Jj
        }));
        Self
    }
}
impl Drop for Pin {
    fn drop(&mut self) {
        crate::vcs::set_preference_for_test(None);
    }
}

/// `pair` of `vcs_log.rs`: open the project, then cycle to the log with Tab.
fn open_log(
    c: &mut Ctx,
    root: &std::path::Path,
    id: &str,
    lang: &str,
    align: &str,
    w: u16,
    h: u16,
) {
    let mut cfg = plugin_config(id, &format!("vcs_log_parity_{id}"), lang);
    cfg.ui.commit_meta_align = align.into();
    cfg.plugins.entry(id.into()).or_default().insert(
        "commit_meta_align".into(),
        toml::Value::String(align.into()),
    );
    c.open(root, id, cfg, w, h);
    c.tab();
}

fn selected_id(c: &mut Ctx) -> String {
    let old = c.is_old();
    let sim = c.sim();
    if old {
        sim.app.git_log_selected_id().unwrap()
    } else {
        sim.app.project_act_context().selected.unwrap().value
    }
}

/// Number of log entries of the old hub (asserted on the old side, constant on the new side).
fn expect_entries(c: &mut Ctx, expected: usize) {
    c.on_old(|old| assert_eq!(old.app.git_log_entries().len(), expected));
}

/// The keys of the detail screen, as the old tests press them.
const DETAIL_KEYS: [(KeyCode, bool); 19] = [
    (KeyCode::Char('j'), false),
    (KeyCode::Char('j'), false),
    (KeyCode::Char('j'), false),
    (KeyCode::Char('d'), true),
    (KeyCode::Char('G'), false),
    (KeyCode::Char('l'), false),
    (KeyCode::Char('l'), false),
    (KeyCode::Char('s'), false),
    (KeyCode::Char('l'), false),
    (KeyCode::Char('0'), false),
    (KeyCode::Char('$'), false),
    (KeyCode::Char('h'), false),
    (KeyCode::Char('g'), false),
    (KeyCode::PageDown, false),
    (KeyCode::PageUp, false),
    // The last four are Ctrl in the vim scheme and plain in the less scheme.
    (KeyCode::Char('d'), true),
    (KeyCode::Char('u'), true),
    (KeyCode::Char('f'), true),
    (KeyCode::Char('b'), true),
];

pub(super) fn detail_cells(c: &mut Ctx) {
    let dir = detail_repo();
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            for empty in [false, true] {
                for scheme in ["vim", "less"] {
                    let height = 14;
                    let mut cfg = plugin_config("git", "vcs_detail_cells", "en");
                    cfg.ui.keys = scheme.into();
                    cfg.git.diff = layout.into();
                    c.open(&dir, "git", cfg, width, height);
                    c.tab();
                    if !empty {
                        c.key('j');
                    }
                    c.enter();
                    let label = format!("w={width} layout={layout} empty={empty} keys={scheme}");
                    c.body(&format!("{label} open"), width, height);
                    for (i, (code, ctrl)) in DETAIL_KEYS.iter().enumerate() {
                        let modifiers = if *ctrl && (i < 15 || scheme == "vim") {
                            KeyModifiers::CONTROL
                        } else {
                            KeyModifiers::NONE
                        };
                        c.press(*code, modifiers);
                        c.body(&format!("{label} key{i}"), width, height);
                    }
                }
            }
        }
    }
}

pub(super) fn detail_missing(c: &mut Ctx) {
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            let dir = detail_repo();
            let mut cfg = plugin_config("git", "vcs_detail_missing", "en");
            cfg.git.diff = layout.into();
            c.open(&dir, "git", cfg, width, 12);
            c.tab();
            let id = selected_id(c);
            c.enter();
            std::fs::remove_file(dir.join(".git/objects").join(&id[..2]).join(&id[2..])).unwrap();
            c.on_old(|old| {
                old.app.close_git_detail();
                old.enter();
            });
            c.on_new(|new| new.key('r'));
            let label = format!("w={width} layout={layout}");
            c.body(&format!("{label} open"), width, 12);
            for (i, (code, ctrl)) in DETAIL_KEYS.iter().take(10).enumerate() {
                let modifiers = if *ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                };
                c.press(*code, modifiers);
                c.body(&format!("{label} key{i}"), width, 12);
            }
        }
    }
}

pub(super) fn git_cells(c: &mut Ctx) {
    use unicode_width::UnicodeWidthStr;
    let dir = repo(13);
    let latest = &crate::git::log(&dir, 200)[0];
    let meta = format!("{} · 2020-01-01 00:12", latest.author);
    let mw = meta.width() as u16;
    for lang in ["en", "ja"] {
        for align in ["right", "inline", "unknown"] {
            // The widths include the old hub's two border columns; mw+2 is the exact fit threshold.
            for (name, width) in [
                ("mw+3", mw + 3),
                ("mw+4", mw + 4),
                ("12", 12),
                ("58", 58),
                ("220", 220),
            ] {
                let height = 8;
                open_log(c, &dir, "git", lang, align, width, height);
                let label = format!("lang={lang} align={align} width={name}");
                c.body(&format!("{label} open"), width, height);
                for (i, code) in [
                    KeyCode::Char('j'),
                    KeyCode::Char('G'),
                    KeyCode::Char('k'),
                    KeyCode::Char('g'),
                    KeyCode::PageDown,
                    KeyCode::PageUp,
                    KeyCode::End,
                    KeyCode::Home,
                    KeyCode::Down,
                    KeyCode::Up,
                ]
                .into_iter()
                .enumerate()
                {
                    // The old hub has no PageUp/PageDown binding; it moves to the same cursor by API.
                    match code {
                        KeyCode::PageDown => c.on_old(|old| old.app.git_log_move(10)),
                        KeyCode::PageUp => c.on_old(|old| old.app.git_log_move(-10)),
                        KeyCode::Home => c.on_old(|old| old.app.git_log_move(-1000)),
                        KeyCode::End => c.on_old(|old| old.app.git_log_move(1000)),
                        _ => c.on_old(|old| old.press(code, KeyModifiers::NONE)),
                    }
                    c.on_new(|new| new.press(code, KeyModifiers::NONE));
                    c.body(&format!("{label} key{i}"), width, height);
                }
            }
        }
    }
}

pub(super) fn filter(c: &mut Ctx) {
    let dir = repo(13);
    let short = crate::git::log(&dir, 1)[0].short.clone();
    for align in ["right", "inline"] {
        for (i, query) in [
            "長い件名",
            "作者",
            "Author",
            "2020-01-01 00:12",
            short.as_str(),
            "Summary 作者",
            "·",
            "SUMMARY",
            "summary",
        ]
        .into_iter()
        .enumerate()
        {
            open_log(c, &dir, "git", "en", align, 100, 8);
            c.raw_key('/');
            for key in query.chars() {
                c.raw_key(key);
            }
            c.sim().enter();
            c.settle();
            let label = format!("align={align} query{i}={query:?}");
            c.body(&format!("{label} filtered"), 100, 8);
            c.key('j');
            c.body(&format!("{label} moved"), 100, 8);
        }
    }
}

pub(super) fn unborn(c: &mut Ctx) {
    for lang in ["en", "ja"] {
        let dir = repo(1);
        let cfg = plugin_config("git", "vcs_log_empty", lang);
        let delete_head = || {
            let repository = git2::Repository::open(&dir).unwrap();
            let head = repository.head().unwrap().name().unwrap().to_string();
            repository.find_reference(&head).unwrap().delete().unwrap();
        };
        if c.is_old() {
            // Remove the only ref after opening changes, making HEAD unborn before cycling.
            let mut old = Sim::with_config_sized(&dir, cfg, 80, 8);
            old.app.open_git_view();
            delete_head();
            old.press(KeyCode::Tab, KeyModifiers::NONE);
            assert!(!old.app.is_git_log());
            assert!(old.app.flash.is_some());
            old.app.show_empty_git_log_for_test();
            c.set_sim(old);
        } else {
            delete_head();
            c.open(&dir, "git", cfg, 80, 8);
            c.tab();
        }
        c.body(&format!("lang={lang}"), 80, 8);
    }
}

pub(super) fn limit(c: &mut Ctx) {
    let dir = repo(205);
    open_log(c, &dir, "git", "en", "right", 100, 8);
    c.key('G');
    c.body("last row of 200", 100, 8);
}

/// Entries of the jj log fixture: the old test shows every entry once.
pub(super) const fn jj_entries(colocated: bool, git: bool) -> usize {
    match (colocated, git) {
        (false, _) => 3,
        (true, true) => 6,
        (true, false) => 8,
    }
}

pub(super) fn jj_log(c: &mut Ctx) {
    for colocated in [false, true] {
        seed_jj_ids(Some(1));
        let dir = jj_repo(colocated);
        seed_jj_ids(None);
        for id in if colocated {
            vec!["git", "jj"]
        } else {
            vec!["jj"]
        } {
            let _pin = Pin::new(id);
            let label = format!("colocated={colocated} backend={id}");
            if id == "jj" {
                open_log(c, &dir, id, "en", "right", 80, 12);
                c.enter();
                c.body(&format!("{label} detail"), 80, 12);
            }
            for align in ["right", "inline", "unknown"] {
                for lang in ["en", "ja"] {
                    open_log(c, &dir, id, lang, align, 72, 7);
                    expect_entries(c, jj_entries(colocated, id == "git"));
                    for i in 0..jj_entries(colocated, id == "git") {
                        c.body(&format!("{label} align={align} lang={lang} row{i}"), 72, 7);
                        c.key('j');
                    }
                }
            }
        }
    }
}
