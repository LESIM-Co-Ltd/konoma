//! The operations (`crate::ops`) through the real key path: a fixed key runs through the same
//! guards as a bound one, a hint is shown only while its key acts, and the notation on screen
//! follows `[keys]` (plugin stage 2 / B2).

use super::*;

/// A config with `table` (a `[keys.<table>]` name) holding the given `key = action` pairs.
fn cfg_keys(table: &str, pairs: &[(&str, &str)]) -> Config {
    let mut cfg = cfg_en();
    let mut t = std::collections::HashMap::new();
    for (k, a) in pairs {
        t.insert((*k).to_string(), (*a).to_string());
    }
    cfg.keys.surfaces.insert(table.to_string(), t);
    cfg
}

/// `Enter` on a jj bookmark list used to reach the checkout: it was handled outside
/// `dispatch_action`, so the read-only guard never saw it (`l` was refused, `Enter` was not).
#[cfg(feature = "git")]
#[test]
fn e2e_enter_on_a_jj_bookmark_list_is_refused_like_every_other_write() {
    let Some(dir) = jj_seed_repo("ops_jj_enter_checkout") else {
        return;
    };
    let root = canon(&dir);
    let mut s = Sim::new(&root).with_async_git_ops();
    assert!(
        !crate::vcs::caps(&s.app.tab.root).write,
        "sanity: this fixture must resolve to the read-only jj backend"
    );
    s.key('p');
    s.key('b');
    assert!(
        s.app.is_git_branches(),
        "the bookmark fixture opens the list"
    );
    // The footer of the same list does not offer the checkout (checked before the refusal's
    // flash takes the footer's place).
    s.see("/:search");
    s.dont_see("checkout");
    s.app.flash = None;
    s.enter();
    assert!(
        s.app.flash.as_deref().is_some_and(
            |f| f.contains(crate::i18n::tr(s.app.lang, crate::i18n::Msg::VcsReadOnly))
        ),
        "Enter on a bookmark must be refused with the read-only flash, got {:?}",
        s.app.flash
    );
    assert!(
        s.app.busy_jobs().is_empty(),
        "a refused Enter must not start a checkout: {:?}",
        s.app.busy_jobs()
    );
    assert!(
        s.app.is_git_branches(),
        "a refused write must not leave the list"
    );
}

/// The contrast: git keeps its `Enter` = checkout.
#[cfg(feature = "git")]
#[test]
fn e2e_enter_on_a_git_branch_list_still_checks_out() {
    let dir = sandbox("ops_git_enter_checkout");
    seed_repo(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root).with_async_git_ops();
    s.key('p');
    s.key('b');
    assert!(s.app.is_git_branches());
    // Move onto `feature-x`, the branch that is not checked out (the list starts on the current one).
    let want = s
        .app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature-x")
        .expect("seed_repo creates feature-x");
    while s.app.git_branch_sel() != want {
        s.key(if s.app.git_branch_sel() > want {
            'k'
        } else {
            'j'
        });
    }
    s.see("l/Enter:checkout");
    s.enter();
    s.drain_git_ops();
    let head = std::process::Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        "feature-x",
        "Enter still checks the branch out on git"
    );
}

/// `p` outside a repository: the footer does not offer it and pressing it says why.
#[test]
fn e2e_git_hub_key_is_not_offered_outside_a_repository() {
    let dir = sandbox("ops_o_outside_repo");
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    // Wait for the first status scan to land, as it does within a frame or two in the app.
    s.app.ensure_git_status_now();
    s.draw();
    s.dont_see("p:git");
    s.key('p');
    assert!(!s.app.is_git_view());
    assert!(
        s.app.flash.as_deref().is_some_and(
            |f| f.contains(crate::i18n::tr(s.app.lang, crate::i18n::Msg::NotAGitRepo))
        ),
        "the refusal keeps the handler's own words: {:?}",
        s.app.flash
    );
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_hub_key_and_diff_key_are_offered_inside_a_repository() {
    let dir = sandbox("ops_o_inside_repo");
    seed_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.app.ensure_git_status_now();
    s.draw();
    s.see("p:git");
    s.select("new.txt");
    s.see("d:diff");
    s.select("b.rs"); // unchanged
    s.dont_see("d:diff");
    s.key('d');
    assert!(
        !s.app.is_git_diff_preview(),
        "d on an unchanged file opens nothing"
    );
    assert!(s.app.flash.is_some(), "and says why");
}

/// Moving a key moves it on screen: in the footer and in the `?` help.
#[test]
fn e2e_a_rebound_key_is_what_the_footer_and_the_help_show() {
    let dir = sandbox("ops_rebind_screen");
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    std::fs::write(dir.join(".hidden"), "x\n").unwrap();
    let cfg = cfg_keys("tree", &[(",", "toggle_hidden"), (".", "noop")]);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 200, 50);
    s.see(",:hidden");
    s.dont_see(".:hidden");
    // The key does what the footer says.
    s.dont_see(".hidden");
    s.key(',');
    s.see(".hidden");
    s.key('.');
    s.see(".hidden"); // `.` is unbound now: nothing changes
    s.key('?');
    let help = s.screen();
    assert!(
        help.lines()
            .any(|l| l.contains("  ,") && l.contains("toggle hidden")),
        "the help names the new key: {help}"
    );
}

/// The fixed keys still do what they did on every screen, now through `dispatch_action`.
#[test]
fn e2e_fixed_keys_keep_their_meaning_on_the_screens() {
    let dir = sandbox("ops_fixed_keys");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/in.txt"), "inner\n").unwrap();
    std::fs::write(dir.join("a.md"), "# t\n\n- [ ] one\n\n[l](b.md)\n").unwrap();
    std::fs::write(dir.join("b.md"), "# b\n").unwrap();
    std::fs::write(dir.join("d.csv"), "a,b\n1,2\n").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    // Tree: Enter expands a directory, Esc clears.
    s.select("sub");
    s.enter();
    s.see("in.txt");
    // Table: Tab does nothing (no panic, no mode change), Enter opens the cell popup, Esc closes it.
    s.select("d.csv");
    s.enter();
    assert!(s.app.is_table_preview());
    s.tab();
    assert!(s.app.is_table_preview());
    s.enter();
    assert_eq!(s.app.key_context(), Some(crate::keymap::Context::TableCell));
    s.esc();
    assert_eq!(s.app.key_context(), Some(crate::keymap::Context::Table));
    s.key('q');
    // Markdown: Tab focuses the first item, Enter on a checkbox toggles it (writes the file) and
    // Space does not, Esc goes back to the tree.
    s.select("a.md");
    s.enter();
    s.tab();
    assert!(s.app.focused_item().is_some(), "Tab focuses the checkbox");
    s.key(' ');
    let src = std::fs::read_to_string(dir.join("a.md")).unwrap();
    assert!(
        src.contains("- [ ] one"),
        "Space is not the toggle any more: {src}"
    );
    s.enter();
    let src = std::fs::read_to_string(dir.join("a.md")).unwrap();
    assert!(
        src.contains("- [x] one"),
        "Enter toggled the focused checkbox: {src}"
    );
    s.esc();
    assert_eq!(s.app.tab.mode, Mode::Tree);
}

/// With a dump path in `KONOMA_DUMP_HELP`, writes the footer and the `?` help of the main screens
/// as text (for the review of the notation); a plain test run does nothing.
#[test]
fn e2e_dump_footer_and_help_when_asked() {
    let Ok(path) = std::env::var("KONOMA_DUMP_HELP") else {
        return;
    };
    let mut out = String::new();
    let mut shot = |label: &str, s: &mut Sim| {
        s.draw();
        out.push_str(&format!("===== {label}: footer =====\n"));
        let rows = s.screen();
        out.push_str(rows.lines().last().unwrap_or(""));
        out.push('\n');
        s.key('?');
        out.push_str(&format!("===== {label}: help =====\n"));
        out.push_str(&s.screen());
        out.push('\n');
        s.key('?');
    };
    let dir = sandbox("ops_dump");
    #[cfg(feature = "git")]
    seed_repo(&dir);
    #[cfg(not(feature = "git"))]
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.join("t.txt"), "plain text\n").unwrap();
    std::fs::write(dir.join("m.md"), "# Title\n\n- [ ] task\n\n[link](a.rs)\n").unwrap();
    std::fs::write(dir.join("d.csv"), "a,b\n1,2\n").unwrap();
    write_solid_png(&dir.join("p.png"), 8, 8, [10, 20, 30]);
    let root = canon(&dir);
    // The hub's `g` / `l` have no default key: bind them to reach the graph and the log.
    #[cfg_attr(not(feature = "git"), allow(unused_mut))]
    let mut cfg = cfg_en();
    #[cfg(feature = "git")]
    hub_screen_keys(&mut cfg);
    let mut s = Sim::with_config_sized(&root, cfg, 200, 130)
        .with_picker()
        .with_media();
    shot("tree", &mut s);
    for (label, file) in [
        ("text", "t.txt"),
        ("markdown", "m.md"),
        ("image", "p.png"),
        ("table", "d.csv"),
    ] {
        s.select(file);
        s.enter();
        if file == "p.png" {
            s.drain_media();
        }
        shot(label, &mut s);
        s.key('q');
    }
    #[cfg(feature = "git")]
    {
        s.select("a.rs");
        s.key('d');
        shot("diff", &mut s);
        s.key('q');
        s.key('p');
        shot("git hub", &mut s);
        s.key('l');
        shot("git log", &mut s);
        s.key('q');
        s.key('g');
        shot("git graph", &mut s);
        s.key('q');
        s.key('b');
        shot("git branches", &mut s);
    }
    std::fs::write(path, out).unwrap();
}
