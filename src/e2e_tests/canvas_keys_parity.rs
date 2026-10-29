//! The keys of a plugin's canvas in the konoma host and in the standalone app (`konoma-plugin`
//! feature `app`). The host gives a canvas its own key context (`CoreContext::Canvas`: no cursor
//! keys, the keys that leave and open the help, the menus), hands every other key to the plugin,
//! and does nothing with a key the plugin does not declare. Given the same plugin, the same size
//! and the same keys, the two must show the same footer, list the same help rows, and show the
//! same screen after each key.
//!
//! The plugin here declares no action and no text input, so every key it is handed is a key that
//! does nothing: the two screens must agree. What a plugin does with a key it declares (an action
//! on `j`, a text input) is the host's alone for now (the app forwards no key to the session).

use super::canvas::{Draw, Painter};
use super::image_parity::{open_nested, WhenIdle};
use super::table_parity::{
    assert_all_seen, hints, panel_lines, rows_of, same_footer, same_help_rows, APP_H, HOST_H, W,
};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, run_with_pictures, AppCx, AppEvent, AppKeys, Display,
};
use konoma_plugin::{Plugin, TermCaps};
use konoma_test_support::TermScreen;
use std::sync::Arc;

const NAME: &str = "canvas-keys-parity";

fn painter() -> Arc<dyn Plugin> {
    Arc::new(Painter::new(NAME, Draw::Pattern { cols: 40, rows: 8 }))
}

/// One key press from a token (the shared tokens, and `<Sp>` for the space bar).
fn kt(t: &str) -> (KeyCode, KeyModifiers) {
    match t {
        "<Sp>" => (KeyCode::Char(' '), KeyModifiers::NONE),
        t => super::table_parity::token(t),
    }
}

/// The host on the canvas of `a.pnt` after the keys (on a cleared screen): its rows, and whether
/// the whole program ended.
fn host(root: &std::path::Path, h: u16, keys: &[&str]) -> (Vec<String>, bool) {
    host_with(root, h, keys, "")
}

/// [`host`] with the `[keys]` of the config `toml`.
fn host_with(root: &std::path::Path, h: u16, keys: &[&str], toml: &str) -> (Vec<String>, bool) {
    let marker = crate::plugins::test_plugins::register(NAME, painter());
    let mut cfg = crate::plugins::test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.keys = toml::from_str::<crate::config::Config>(toml)
        .expect("a config")
        .keys;
    let mut s = Sim::with_config_sized(root, cfg, W, h);
    open_nested(&mut s, "a.pnt");
    s.draw();
    for k in keys {
        let (c, m) = kt(k);
        s.press(c, m);
    }
    s.term.clear().expect("clear");
    s.draw();
    (rows_of(&s.term), s.quit)
}

/// The app on `a.pnt` after the keys: its rows, and whether the loop ended before the last key
/// (a sentinel key `z`, which does nothing on a canvas, is left unread when it did).
fn app(root: &std::path::Path, h: u16, keys: &[&str]) -> (Vec<String>, bool) {
    app_with(root, h, keys, true, "")
}

/// [`app`] with or without the sentinel (a menu waiting for its key takes the sentinel as its
/// key, so a menu is looked at without one, and `ended` is then always false).
fn app_with(
    root: &std::path::Path,
    h: u16,
    keys: &[&str],
    sentinel: bool,
    toml: &str,
) -> (Vec<String>, bool) {
    let o =
        konoma_plugin::app::open_file(vec![painter()], &root.join("a.pnt"), "en", TermCaps::new())
            .expect("opens");
    let mut session = o.session;
    let mut term = Terminal::new(TermScreen::new(W, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        unavailable: o.unavailable.clone(),
        shows: o.shows.clone(),
        path: o.path.clone(),
        hint: o.hint.clone(),
        open_dir: Some(root.to_path_buf()),
        display: Display::default(),
        ..AppCx::default()
    };
    let mut evs: std::collections::VecDeque<AppEvent> = keys
        .iter()
        .chain(sentinel.then_some(&"z"))
        .map(|k| {
            let (c, m) = kt(k);
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(c, m)).expect("a key"))
        })
        .collect();
    let mut ev = WhenIdle {
        keys: std::mem::take(&mut evs),
        turns: 0,
    };
    run_with_pictures(
        &mut term,
        &mut *session,
        &mut ev,
        &cx,
        &AppKeys::new(
            &toml::from_str::<crate::config::Config>(toml)
                .expect("a config")
                .keys,
            "vim",
        ),
        None,
    )
    .unwrap();
    let ended = !ev.keys.is_empty();
    let buf = term.backend().1.clone();
    let mut shown = Terminal::new(ratatui::backend::TestBackend::new(
        buf.area.width,
        buf.area.height,
    ))
    .unwrap();
    shown.draw(|f| *f.buffer_mut() = buf.clone()).expect("draw");
    (rows_of(&shown), ended)
}

fn fixture() -> (crate::test_support::TmpDir, std::path::PathBuf) {
    let dir = sandbox("canvas-keys-parity");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let root = canon(&dir);
    (dir, root)
}

/// The canvas's own row ("n=0 ..."), which both draw the same.
fn canvas_row(rows: &[String]) -> Option<String> {
    // The host frames the canvas in a border; the cells inside are the same.
    rows.iter().find_map(|r| {
        r.find("n=")
            .map(|at| r[at..].trim_end_matches(['│', ' ']).to_string())
    })
}

#[test]
fn the_footer_of_a_canvas_agrees_with_the_host() {
    let (_dir, root) = fixture();
    let (hr, _) = host(&root, HOST_H, &[]);
    let (ar, ended) = app(&root, APP_H, &[]);
    assert!(!ended);
    assert!(
        canvas_row(&hr).is_some(),
        "the host shows the canvas: {hr:?}"
    );
    assert!(
        canvas_row(&ar).is_some(),
        "the app shows the canvas: {ar:?}"
    );
    let hh: Vec<String> = hints(hr.last().unwrap())
        .into_iter()
        // The app says `quit` where the host says `back` (it has no tree to go back to).
        .map(|t| {
            if t == "q:back" {
                "q:quit".to_string()
            } else {
                t
            }
        })
        .collect();
    let ah = hints(ar.last().unwrap());
    // What only the host has: the file jump, the editor, the opener, the tabs.
    const HOST_ONLY: [&str; 4] = [
        "C-n/C-p:next/prev file",
        "e:edit",
        "o:open in app",
        "[/]:tab",
        ];
    let (alone, compared) = same_footer("canvas footer", &hh, &ah, &HOST_ONLY, &|_| false);
    let seen: std::collections::BTreeSet<String> = alone.into_iter().collect();
    assert_all_seen("HOST_ONLY", &HOST_ONLY, &seen);
    // The two shared hints (leave, help) were compared.
    assert!(compared >= 2, "{compared}");
    assert!(ah.len() >= 2, "{ah:?}");
}

#[test]
fn the_help_of_a_canvas_agrees_with_the_host() {
    let (_dir, root) = fixture();
    let (hr, _) = host(&root, 130, &["?"]);
    let (ar, _) = app(&root, 129, &["?"]);
    let (hl, al) = (panel_lines(&hr), panel_lines(&ar));
    // The help is the canvas context's (the host's `CoreContext::Canvas`), not a text's.
    for (who, lines) in [("host", &hl), ("app", &al)] {
        let all = lines.join("\n");
        assert!(all.contains("Preview: plugin view"), "{who} help: {all}");
        assert!(
            !all.contains("Preview: text"),
            "{who} help is a text's: {all}"
        );
    }
    // The rows only the host has, by their start: the launch keys, the menus, the tabs, the
    // file jump, the `@path` copy, the paste and the follow mode.
    const HOST_ONLY: [&str; 16] = [
        "Launch",
        "Actions",
        "Space ",
        "Tabs",
        "t ",
        "[ / ]",
        "1 - 9",
        "T ",
        "P ",
        "F ",
        "Y ",
        "Ctrl-n / Ctrl-p",
        "m / '",
        "r ",
        "e ",
        "o ",
    ];
    let nonblank =
        |v: Vec<String>| -> Vec<String> { v.into_iter().filter(|l| !l.is_empty()).collect() };
    let (hl, al) = (nonblank(hl), nonblank(al));
    let (compared, used) = same_help_rows("canvas help", &hl, &al, &HOST_ONLY);
    let used: std::collections::BTreeSet<String> = used.into_iter().collect();
    // Every entry of the list took a row away.
    assert_all_seen("HOST_ONLY", &HOST_ONLY, &used);
    // Not vacuous: rows were compared, and at least the leave/help/quit rows are among them.
    assert!(compared >= 3, "{compared}\nhost: {hl:#?}\napp: {al:#?}");
}

#[test]
fn each_key_over_a_canvas_leaves_the_same_screen_as_the_host() {
    let (_dir, root) = fixture();
    // Keys that do nothing on a canvas whose plugin declares nothing: the screen stays.
    const STAY: [&str; 31] = [
        "g", "G", "j", "k", "h", "l", "<C-d>", "<C-u>", "<C-f>", "<C-b>", "<PgDn>", "<PgUp>",
        "<Down>", "<Up>", "<Left>", "<Right>", "<Home>", "<End>", "<CR>", "0", "$", "a", "x", "J",
        "K", "n", "N", "/", "w", "d", "u",
    ];
    let (h0, _) = host(&root, HOST_H, &[]);
    let (a0, _) = app(&root, APP_H, &[]);
    assert_eq!(canvas_row(&h0), canvas_row(&a0), "the canvases differ");
    let base = canvas_row(&h0).expect("a canvas row");
    let mut stayed = 0;
    for k in STAY {
        let (hr, hq) = host(&root, HOST_H, &[k]);
        let (ar, aend) = app(&root, APP_H, &[k]);
        assert!(!hq, "the host quit on {k}");
        assert!(!aend, "the app left on {k}");
        assert_eq!(canvas_row(&hr), Some(base.clone()), "host {k}: {hr:?}");
        assert_eq!(canvas_row(&ar), Some(base.clone()), "app {k}: {ar:?}");
        stayed += 1;
    }
    assert!(stayed >= 30, "{stayed}");
    // Keys that leave: the host goes back to the tree (or quits), the app ends.
    let mut left = 0;
    for k in ["q", "<Esc>"] {
        let (hr, hq) = host(&root, HOST_H, &[k]);
        let (_, aend) = app(&root, APP_H, &[k]);
        assert!(aend, "the app stays on {k}");
        assert!(
            hq || canvas_row(&hr).is_none(),
            "the host stays on the canvas on {k}: {hr:?}"
        );
        left += 1;
    }
    assert_eq!(left, 2);
    // `Q` quits everything: the host asks first (`confirm_quit`) and then quits; the app, which
    // has nothing else open, ends.
    let (hr, hq) = host(&root, HOST_H, &["Q"]);
    assert!(
        !hq && hr.iter().any(|r| r.contains("Quit konoma?")),
        "{hr:?}"
    );
    let (_, hq) = host(&root, HOST_H, &["Q", "y"]);
    assert!(hq, "the host did not quit on Q y");
    let (_, aend) = app(&root, APP_H, &["Q"]);
    assert!(aend, "the app stays on Q");
    // `?` opens the help and the next `?`, `q` and `Esc` close it; the canvas is back.
    let mut closed = 0;
    for close in ["?", "q", "<Esc>"] {
        let (hr, hq) = host(&root, HOST_H, &["?", close]);
        let (ar, aend) = app(&root, APP_H, &["?", close]);
        assert!(!hq && !aend, "{close} left instead of closing the help");
        assert_eq!(canvas_row(&hr), Some(base.clone()), "host ? {close}");
        assert_eq!(canvas_row(&ar), Some(base.clone()), "app ? {close}");
        closed += 1;
    }
    assert_eq!(closed, 3);
    // In the help `j` scrolls the help and not the canvas: both leave the canvas row as it was
    // under the panel and stay in the help (the panel hides the canvas on a short screen).
    let (hr, _) = host(&root, HOST_H, &["?", "j", "j"]);
    let (ar, _) = app(&root, APP_H, &["?", "j", "j"]);
    assert!(
        panel_lines(&hr)
            .iter()
            .any(|l| l.contains("Preview: plugin view"))
            || panel_lines(&hr).len() > 3,
        "{hr:?}"
    );
    assert!(panel_lines(&ar).len() > 3, "{ar:?}");
    // `y` opens the copy menu on both.
    let (hr, _) = host(&root, HOST_H, &["y"]);
    let (ar, _) = app_with(&root, APP_H, &["y"], false, "");
    let (hf, af) = (hr.last().unwrap().clone(), ar.last().unwrap().clone());
    assert!(hf.contains("copy") || hf.contains("path"), "{hf}");
    assert_eq!(hf.trim(), af.trim(), "the copy menu line differs");
}

/// A key rebound in another context does not reach a canvas, and one rebound in the canvas's
/// own table does: the keys come from the host's `Canvas` context in both.
#[test]
fn a_rebound_key_reaches_a_canvas_as_it_does_the_host() {
    let (_dir, root) = fixture();
    let toml = "[keys.text]\nx = \"toggle_help\"\nj = \"toggle_help\"\n[keys.canvas]\nw = \"toggle_help\"\n";
    let helped = |rows: &[String]| rows.iter().any(|r| r.contains("Preview: plugin view"));
    let (mut opened, mut ignored) = (0, 0);
    for k in ["x", "j", "w"] {
        let (hr, hq) = host_with(&root, 130, &[k], toml);
        let (ar, aend) = app_with(&root, 129, &[k], true, toml);
        assert!(!hq && !aend, "{k} left");
        assert_eq!(
            helped(&hr),
            helped(&ar),
            "{k}: the help is open on one only"
        );
        if helped(&hr) {
            opened += 1;
            assert_eq!(k, "w", "only the canvas's own binding opens the help");
        } else {
            ignored += 1;
        }
    }
    assert_eq!((opened, ignored), (1, 2));
}
