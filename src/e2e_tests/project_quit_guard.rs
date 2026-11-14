//! Quitting while a project act runs: a running WRITE act warns like the old hub's git write did
//! (`QuitWhileGitOp`), a running READ never does. The host knows a write from the act's own
//! `writes` declaration, so the test plugin's `w` (writes) and `x` (capture, a read) stand for
//! the vcs plugin's commit / checkout / refresh. Order comes from latches, never from the clock.

use super::*;
use crate::i18n::{Lang, Msg};

/// A project open on the first screen with `key`'s act started and held inside the plugin.
fn held(name: &str, key: char, confirm_quit: bool) -> (Fixture, Arc<Latch>) {
    let hold = Arc::new(Latch::default());
    let started = Arc::new(Latch::default());
    let mut f = fixture(
        name,
        false,
        1,
        None,
        Some(hold.clone()),
        Some(started.clone()),
    );
    f.sim.app.cfg.ui.confirm_quit = confirm_quit;
    open(&mut f);
    f.sim.key(key);
    started.wait();
    assert!(started.is_open(), "the act must be inside the plugin");
    assert!(
        f.sim.app.project_jobs_pending(),
        "{key}: the act is running"
    );
    (f, hold)
}

fn warning(f: &Fixture) -> &'static str {
    crate::i18n::tr(f.sim.app.lang, Msg::QuitWhileGitOp)
}

/// The confirmation's text when `Q` opened one, `None` when there is no dialog.
fn quit_dialog(f: &Fixture) -> Option<String> {
    f.sim
        .app
        .dialog_view()
        .map(|(_, message, _, _)| message.to_string())
}

#[test]
fn e2e_quit_project_write_act_running_warns_in_en_and_ja() {
    for lang in [Lang::En, Lang::Jp] {
        // A fixture name is one registration and one sandbox: each pass needs its own.
        let (mut f, hold) = held(&format!("quit_guard_write_{lang:?}"), 'w', true);
        f.sim.app.lang = lang;
        assert!(f.sim.app.project_write_running());
        f.sim.key('Q');
        assert!(!f.sim.quit, "{lang:?}: the dialog asks first");
        let message = quit_dialog(&f).expect("a confirmation");
        assert!(
            message.contains(warning(&f)),
            "{lang:?}: the git-op line is missing: {message}"
        );
        assert!(
            message.contains(crate::i18n::tr(lang, Msg::QuitConfirm)),
            "{lang:?}: {message}"
        );
        hold.open();
        drain(&mut f.sim);
    }
    // The two languages really differ, so the two passes above checked two strings.
    assert_ne!(
        crate::i18n::tr(Lang::En, Msg::QuitWhileGitOp),
        crate::i18n::tr(Lang::Jp, Msg::QuitWhileGitOp)
    );
}

#[test]
fn e2e_quit_project_write_act_y_quits_and_n_keeps_the_act_running_to_completion() {
    let (mut f, hold) = held("quit_guard_write_yn", 'w', true);
    f.sim.key('Q');
    assert!(quit_dialog(&f).is_some_and(|m| m.contains(warning(&f))));
    f.sim.key('n');
    assert!(!f.sim.quit, "n keeps konoma open");
    assert!(quit_dialog(&f).is_none(), "n closes the dialog");
    assert!(
        f.sim.app.project_write_running(),
        "n leaves the write running"
    );
    assert_eq!(f.log.lock().unwrap().acts.len(), 1);
    hold.open();
    drain(&mut f.sim);
    assert!(!f.sim.app.project_write_running(), "the act completed");
    assert_eq!(last_act(&f).0, "write");

    // `y` still leaves: the dialog only warns.
    let (mut g, hold) = held("quit_guard_write_y", 'w', true);
    g.sim.key('Q');
    assert!(quit_dialog(&g).is_some_and(|m| m.contains(warning(&g))));
    g.sim.key('y');
    assert!(g.sim.quit, "y quits during a write");
    hold.open();
    drain(&mut g.sim);
}

#[test]
fn e2e_quit_project_read_act_running_adds_no_git_op_line() {
    let (mut f, hold) = held("quit_guard_read_act", 'x', true);
    assert!(!f.sim.app.project_write_running(), "capture is a read");
    f.sim.key('Q');
    let message = quit_dialog(&f).expect("confirm_quit asks anyway");
    assert!(
        !message.contains(warning(&f)),
        "a read must not warn: {message}"
    );
    f.sim.key('n');
    hold.open();
    drain(&mut f.sim);

    // With confirm_quit off a running read quits at once, like an idle app.
    let (mut g, hold) = held("quit_guard_read_act_off", 'x', false);
    g.sim.key('Q');
    assert!(quit_dialog(&g).is_none(), "no dialog for a read");
    assert!(g.sim.quit, "a read does not hold quit back");
    hold.open();
    drain(&mut g.sim);
}

#[test]
fn e2e_quit_project_view_refresh_running_adds_no_git_op_line() {
    // `r` re-reads the screen: the owner is busy but no act runs.
    let mut f = fixture("quit_guard_refresh", false, 1, None, None, None);
    f.sim.app.cfg.ui.confirm_quit = true;
    open(&mut f);
    let gate = ViewHold::new("items");
    f.log.lock().unwrap().view_hold = Some(gate.clone());
    f.sim.key('r');
    gate.entered.wait();
    assert!(f.sim.app.project_jobs_pending(), "the refresh is running");
    assert!(!f.sim.app.project_write_running());
    f.sim.key('Q');
    let message = quit_dialog(&f).expect("confirm_quit asks");
    assert!(!message.contains(warning(&f)), "{message}");
    f.sim.key('n');
    f.sim.app.cfg.ui.confirm_quit = false;
    f.sim.key('Q');
    assert!(
        f.sim.quit,
        "no dialog at all for a read with confirm_quit off"
    );
    gate.release.open();
    drain(&mut f.sim);
}

#[test]
fn e2e_quit_project_write_act_dialog_shows_even_with_confirm_quit_off() {
    let (mut f, hold) = held("quit_guard_write_off", 'w', false);
    f.sim.key('Q');
    assert!(!f.sim.quit, "a write holds quit back behind the dialog");
    let message = quit_dialog(&f).expect("the dialog is shown regardless of the setting");
    assert!(message.contains(warning(&f)), "{message}");
    f.sim.key('n');
    hold.open();
    drain(&mut f.sim);
}

#[test]
fn e2e_quit_after_the_write_finished_adds_no_git_op_line() {
    let (mut f, hold) = held("quit_guard_write_done", 'w', true);
    hold.open();
    drain(&mut f.sim);
    assert!(!f.sim.app.project_write_running());
    f.sim.key('Q');
    let message = quit_dialog(&f).expect("confirm_quit asks");
    assert!(!message.contains(warning(&f)), "{message}");
    f.sim.key('n');

    let (mut g, hold) = held("quit_guard_write_done_off", 'w', false);
    hold.open();
    drain(&mut g.sim);
    g.sim.key('Q');
    assert!(g.sim.quit, "idle again: confirm_quit off quits at once");
}

#[test]
fn e2e_quit_write_act_of_a_background_tab_still_warns() {
    // The runtime is the project's, not the tab's: a second tab sees the same running write.
    let (mut f, hold) = held("quit_guard_write_tab", 'w', true);
    f.sim.key('t');
    f.sim.app.tab.projects.wait_for_result();
    f.sim.app.poll_projects();
    assert!(f.sim.app.project_write_running());
    f.sim.key('Q');
    assert!(quit_dialog(&f).is_some_and(|m| m.contains(warning(&f))));
    f.sim.key('n');
    hold.open();
    drain(&mut f.sim);
}
