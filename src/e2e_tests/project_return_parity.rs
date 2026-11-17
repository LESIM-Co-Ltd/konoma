//! What a preview is put back to when a project screen that covered it is left, through the real
//! keys (`p`, then `q`) and the project runtime, and through a tab switch. The restore
//! (`App::put_tab_back`) puts the view back and brings it up to date with the files as they are
//! now, and never reverts what moved on while the screen was up.

use super::*;

fn road(root: &std::path::Path, cfg: Config, worker: bool) -> Sim {
    let base = Sim::with_config_sized(root, cfg, 100, 30);
    if worker {
        base.with_media_and_preview_worker()
    } else {
        base.with_media()
    }
}

/// A git repository (so `p` opens a project) holding `files`, committed.
fn repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = sandbox(name);
    init_git_repo(&dir);
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    let root = canon(&dir);
    std::mem::forget(dir); // the guard must outlive both roads
    root
}

fn settle_projects(s: &mut Sim) {
    if s.app.tab.projects.pending() {
        s.app.tab.projects.wait_for_result();
    }
    for _ in 0..32 {
        s.app.poll_projects();
        if !s.app.project_jobs_pending() {
            s.settle_inline_images("projects", std::time::Duration::from_secs(120));
            s.draw();
            return;
        }
        s.app.wait_project_job();
    }
    panic!("the project did not settle");
}

fn settled(s: &mut Sim) {
    s.settle_inline_images("settled", std::time::Duration::from_secs(120));
    s.draw();
}

#[track_caller]
fn open_project(s: &mut Sim) {
    s.key('p');
    settle_projects(s);
    assert!(
        s.app.project_active(),
        "the project screen is up:\n{}",
        s.screen()
    );
}

#[track_caller]
fn leave_project(s: &mut Sim) {
    s.key('q');
    settle_projects(s);
    assert!(
        !s.app.project_active(),
        "the project screen is gone:\n{}",
        s.screen()
    );
}

/// The marker of the project is removed while its screen covers the preview; detection answers
/// again.
fn lose_marker(s: &mut Sim, root: &std::path::Path, marker: &str) {
    std::fs::remove_dir_all(root.join(marker)).unwrap();
    s.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
            notify::event::RemoveKind::Any,
        ))
        .add_path(root.join(marker))));
    s.app.poll_projects();
    settle_projects_allow_closed(s);
}

fn settle_projects_allow_closed(s: &mut Sim) {
    if s.app.tab.projects.pending() {
        s.app.tab.projects.wait_for_result();
    }
    for _ in 0..32 {
        s.app.poll_projects();
        if !s.app.project_jobs_pending() {
            settled(s);
            return;
        }
        s.app.wait_project_job();
    }
    panic!("detection did not settle");
}

fn wait_diff_target(s: &mut Sim, worker: bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while worker && !s.app.diff_target_resolved_for_test() {
        let left = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("the diff view was never decided");
        let res = s
            .media_rx
            .as_ref()
            .expect("with_media")
            .recv_timeout(left)
            .expect("the worker answers");
        s.app.apply_media(res);
    }
}

/// How the tab comes back, on both roads: through a project screen (`p`, then `q`) or through a
/// tab switch (a new tab, then back), with `change` made to the files while the tab is away.
#[derive(Clone, Copy, Debug)]
enum Away {
    ProjectScreen,
    Tab,
}

fn go_away(s: &mut Sim, away: Away) {
    match away {
        Away::ProjectScreen => open_project(s),
        Away::Tab => {
            s.key('t');
            settled(s);
        }
    }
}

fn come_back(s: &mut Sim, away: Away) {
    match away {
        Away::ProjectScreen => leave_project(s),
        Away::Tab => {
            s.key('[');
            settled(s);
        }
    }
}

const BOTH: [Away; 2] = [Away::ProjectScreen, Away::Tab];

// --- (a) A project lost while it covers a preview does not come back with the preview.

#[test]
fn a_project_lost_while_it_covers_a_preview_does_not_come_back_with_the_preview() {
    use crate::i18n::{tr, Lang, Msg};
    for worker in [false, true] {
        let root = repo(
            "prp_lost",
            &[("a.md", "# A\n\ntext\n"), ("b.md", "# B\n\nbee\n")],
        );
        std::fs::write(root.join("b.md"), "# B\n\nbee changed\n").unwrap();
        let mut s = road(&root, cfg_en(), worker);
        s.select("a.md");
        s.enter();
        settled(&mut s);
        open_project(&mut s);
        // A file opened out of the project screen: the preview now has a way back to it.
        s.enter();
        settle_projects(&mut s);
        assert!(s.app.tab.project_return.is_some(), "worker={worker}");
        // The screen is opened again over that preview, and the project is lost under it.
        s.key('p');
        settle_projects(&mut s);
        assert!(s.app.project_active(), "worker={worker}");
        lose_marker(&mut s, &root, ".git");
        let what = format!("worker={worker}\n{}", s.screen());
        assert!(!s.app.project_active(), "{what}");
        assert_eq!(s.app.tab.mode, Mode::Preview, "{what}");
        assert!(s.app.tab.projects.projects.is_empty(), "{what}");
        assert!(
            s.app.tab.project_return.is_none(),
            "the way back to the lost project came back: {what}"
        );
        assert!(s.app.tab.project_navigation.is_none(), "{what}");
        assert_eq!(
            s.app.flash.as_deref(),
            Some(tr(Lang::En, Msg::NotAGitRepo)),
            "{what}"
        );
    }
}

// --- (b) A command's output is the output of the run that was made now.

fn command_cfg() -> Config {
    let mut cfg = cfg_en();
    cfg.preview.rules = vec![crate::config::Rule {
        glob: Some("*.custom".into()),
        command: Some("cat {path}".into()),
        ..Default::default()
    }];
    cfg
}

#[test]
fn a_command_output_read_halfway_is_put_back_on_the_same_line_of_the_new_output() {
    for worker in [false, true] {
        for away in BOTH {
            let plain: String = (1..=300).map(|i| format!("line number {i}\n")).collect();
            let root = repo("prp_command", &[("note.custom", &plain)]);
            let mut s = road(&root, command_cfg(), worker);
            s.select("note.custom");
            s.enter();
            settled(&mut s);
            s.app.paste_jump_to(&root.join("note.custom"), Some(150));
            settled(&mut s);
            assert_eq!(s.app.preview_caret_line(), 149, "{away:?} worker={worker}");
            go_away(&mut s, away);
            // Every line is longer, so no byte of the old output is a line head in the new.
            let longer: String = (1..=300)
                .map(|i| format!("the new output line number {i} with a longer tail than before\n"))
                .collect();
            std::fs::write(root.join("note.custom"), longer).unwrap();
            come_back(&mut s, away);
            let what = format!("{away:?} worker={worker}\n{}", s.screen());
            assert_eq!(s.app.preview_caret_line(), 149, "{what}");
            s.see("the new output line number 150 with");
            s.dont_see("line number 150\n");
        }
    }
}

// --- (c) A landing that failed while covered is not revived.

#[test]
fn a_line_asked_of_a_document_that_failed_is_not_landed_when_it_is_readable_again() {
    use std::os::unix::fs::PermissionsExt;
    let mut doc = String::from("# Title\n\n");
    for i in 0..60 {
        doc.push_str(&format!("Paragraph {i} with some **bold** words.\n\n"));
    }
    for worker in [false, true] {
        for away in BOTH {
            let root = repo("prp_failed_landing", &[("doc.md", &doc), ("x.txt", "x\n")]);
            let path = root.join("doc.md");
            let mut s = road(&root, cfg_en(), worker);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            s.app.paste_jump_to(&path, Some(40));
            settled(&mut s);
            assert!(
                s.app.document_error().is_some(),
                "{away:?} worker={worker}: the unreadable document must have failed:\n{}",
                s.screen()
            );
            go_away(&mut s, away);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            come_back(&mut s, away);
            let what = format!("{away:?} worker={worker}\n{}", s.screen());
            assert!(s.app.document_error().is_none(), "{what}");
            s.see("Paragraph 0");
            assert_eq!(s.app.preview_top_line(), 0, "{what}");
        }
    }
}

// --- (d) A Markdown diff comes back with its representation and its scroll.

#[test]
fn a_markdown_diff_comes_back_with_its_representation_and_scroll() {
    // Every paragraph is changed, so the diff is longer than the screen in every representation.
    let (mut original, mut changed) = (String::from("# Title\n\n"), String::from("# Title\n\n"));
    for i in 0..80 {
        original.push_str(&format!(
            "Filler paragraph number {i} to take up space.\n\n"
        ));
        changed.push_str(&format!("Filler paragraph number {i} was edited here.\n\n"));
    }
    for worker in [false, true] {
        for away in BOTH {
            let root = repo("prp_md_diff", &[("doc.md", &original)]);
            std::fs::write(root.join("doc.md"), &changed).unwrap();
            let mut s = road(&root, cfg_en(), worker);
            s.select("doc.md");
            s.key('d');
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            let default_view = s.app.diff_view_for_test();
            // R until the representation is not the one a diff opens in.
            for _ in 0..3 {
                if s.app.diff_view_for_test() != default_view {
                    break;
                }
                s.key('R');
                wait_diff_target(&mut s, worker);
                settled(&mut s);
            }
            let view = s.app.diff_view_for_test();
            assert_ne!(
                view, default_view,
                "{away:?} worker={worker}: R did not change it"
            );
            for _ in 0..6 {
                s.key('j');
            }
            s.draw();
            let before = (
                s.app.diff_view_for_test(),
                s.app.diff_view_choice_for_test(),
                s.app.tab.preview_scroll,
                s.app.preview_top_line(),
                s.app.preview_caret_line(),
            );
            assert!(
                before.2 > 0 || before.3 > 0 || before.4 > 0,
                "{away:?} worker={worker}: the diff did not scroll: {before:?}"
            );
            go_away(&mut s, away);
            come_back(&mut s, away);
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            let after = (
                s.app.diff_view_for_test(),
                s.app.diff_view_choice_for_test(),
                s.app.tab.preview_scroll,
                s.app.preview_top_line(),
                s.app.preview_caret_line(),
            );
            assert_eq!(after, before, "{away:?} worker={worker}:\n{}", s.screen());
        }
    }
}

// --- (d2) A diff's Preview representation: a landing still pending is made on the current
// content; a landing that was consumed is not made again, the reader's scroll is put back.

fn long_doc(first: &str, last: &str) -> String {
    let mut body = format!("# Title{first}\n\n");
    for i in 0..80 {
        body.push_str(&format!(
            "Filler paragraph number {i} to take up space.\n\n"
        ));
    }
    format!("{body}{last}\n")
}

/// Waits for the project detection and the project jobs only: the replies of the preview worker
/// are left where they are (a landing waiting for the text must still be waiting).
fn settle_projects_leaving_media(s: &mut Sim) {
    if s.app.tab.projects.pending() {
        s.app.tab.projects.wait_for_result();
    }
    for _ in 0..32 {
        s.app.poll_projects();
        if !s.app.project_jobs_pending() {
            s.draw();
            return;
        }
        s.app.wait_project_job();
    }
    panic!("the project did not settle");
}

/// Opens `doc.md`'s diff and chooses its Preview representation (`R`), with the text of that
/// representation not read yet when the worker reads it.
fn diff_preview_asked(root: &std::path::Path, worker: bool) -> Sim {
    let mut s = road(root, cfg_en(), worker);
    s.select("doc.md");
    s.key('d');
    wait_diff_target(&mut s, worker);
    settled(&mut s);
    s.key('R');
    s
}

/// With no worker the landing of `R` has run, and the draw that follows the key has consumed its
/// reservation, before anything can cover the screen: a landing still pending cannot be reached.
#[test]
fn without_the_worker_a_diff_previews_landing_is_over_before_the_screen_can_be_covered() {
    let root = repo(
        "prp_diff_landing_sync",
        &[("doc.md", &long_doc("", "ORIGINAL MARKER paragraph."))],
    );
    std::fs::write(
        root.join("doc.md"),
        long_doc("", "CHANGED MARKER paragraph."),
    )
    .unwrap();
    let s = diff_preview_asked(&root, false);
    assert!(s.app.preview_is_diff_representation(), "{}", s.screen());
    assert!(
        !s.app.has_pending_landing_for_test(),
        "the landing is still pending"
    );
    assert!(
        !s.app.diff_landing_state_for_test().1,
        "the reservation is still armed"
    );
    s.see("CHANGED MARKER");
}

/// (i) Covered while the landing is still pending (the worker has not answered): back on the first
/// change of the content it has now, without a key.
#[test]
fn a_revived_diff_preview_lands_on_the_first_change_of_the_current_content() {
    for away in BOTH {
        let root = repo(
            "prp_diff_landing_pending",
            &[("doc.md", &long_doc("", "ORIGINAL MARKER paragraph."))],
        );
        std::fs::write(
            root.join("doc.md"),
            long_doc("", "CHANGED MARKER paragraph."),
        )
        .unwrap();
        let mut s = diff_preview_asked(&root, true);
        assert!(
            s.app.diff_landing_state_for_test().0,
            "{away:?}: the landing is not pending:\n{}",
            s.screen()
        );
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => s.key('t'),
        }
        // While covered the first change moves to the top of the file.
        std::fs::write(
            root.join("doc.md"),
            long_doc(" EARLY CHANGE", "CHANGED MARKER paragraph."),
        )
        .unwrap();
        match away {
            Away::ProjectScreen => {
                s.key('q');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => s.key('['),
        }
        settled(&mut s);
        assert!(
            s.find_text("EARLY CHANGE").is_some(),
            "{away:?}: not on the first change of the current content:\n{}",
            s.screen()
        );
    }
}

/// (ii) Covered after the landing was consumed: the reader's scroll is put back as it was
/// (clamped), and what is drawn there is the current content.
#[test]
fn a_diff_preview_whose_landing_was_consumed_keeps_its_scroll_and_shows_the_current_content() {
    for worker in [false, true] {
        for away in BOTH {
            let root = repo(
                "prp_diff_landing_done",
                &[("doc.md", &long_doc("", "ORIGINAL MARKER paragraph."))],
            );
            std::fs::write(
                root.join("doc.md"),
                long_doc("", "CHANGED MARKER paragraph."),
            )
            .unwrap();
            let mut s = diff_preview_asked(&root, worker);
            settled(&mut s);
            assert!(s.app.preview_is_diff_representation(), "{}", s.screen());
            let before = s.app.reading_position_for_test();
            let what = format!("{away:?} worker={worker}");
            assert!(
                before.0 > 0 || before.2 > 0,
                "{what}: not scrolled: {before:?}"
            );
            s.see("CHANGED MARKER");
            s.dont_see("EARLY CHANGE");
            go_away(&mut s, away);
            // The first change moves to the top, and the text at the end is not the same text.
            std::fs::write(
                root.join("doc.md"),
                long_doc(" EARLY CHANGE", "CHANGED MARKER paragraph NEWTAIL."),
            )
            .unwrap();
            come_back(&mut s, away);
            settled(&mut s);
            let what = format!("{what}\n{}", s.screen());
            assert_eq!(s.app.reading_position_for_test(), before, "{what}");
            // The landing is not made again (the reader's scroll stands), and the text drawn
            // there is the content as it is now.
            assert!(
                s.find_text("EARLY CHANGE").is_none(),
                "landed again: {what}"
            );
            assert!(s.find_text("NEWTAIL").is_some(), "old content: {what}");
            s.key('g');
            settled(&mut s);
            assert!(s.find_text("EARLY CHANGE").is_some(), "old content: {what}");
        }
    }
}

// --- (e) A confirmed search is found again on the current text.

#[test]
fn a_confirmed_search_moves_between_matches_of_the_current_text_after_the_return() {
    for worker in [false, true] {
        for away in BOTH {
            let text = |lengthen: bool, hit_gone: bool| -> String {
                (1..=300)
                    .map(|i| {
                        let head = if lengthen && i < 100 {
                            "a much longer head before the hit "
                        } else {
                            ""
                        };
                        let hit = matches!(i, 50 | 200) || (i == 120 && !hit_gone);
                        format!("{head}line {i}{}\n", if hit { " needle" } else { "" })
                    })
                    .collect()
            };
            let root = repo("prp_search", &[("long.txt", &text(false, false))]);
            let mut s = road(&root, cfg_en(), worker);
            s.select("long.txt");
            s.enter();
            settled(&mut s);
            s.key('/');
            for c in "needle".chars() {
                s.key(c);
            }
            s.enter();
            settled(&mut s);
            s.key('n');
            settled(&mut s);
            assert_eq!(
                s.app.search_status().map(|x| x.1),
                Some(3),
                "{away:?} worker={worker}"
            );
            assert_eq!(s.app.preview_top_line(), 119, "{away:?} worker={worker}");
            go_away(&mut s, away);
            std::fs::write(root.join("long.txt"), text(true, true)).unwrap();
            come_back(&mut s, away);
            let what = format!("{away:?} worker={worker}\n{}", s.screen());
            assert_eq!(s.app.search_status().map(|x| x.1), Some(2), "{what}");
            assert_eq!(s.app.preview_search_query(), Some("needle"), "{what}");
            s.key('n');
            settled(&mut s);
            let line = s.app.preview_top_line() + 1;
            assert!(line == 50 || line == 200, "n went to line {line}: {what}");
            s.key('n');
            settled(&mut s);
            let other = s.app.preview_top_line() + 1;
            assert!(
                (other == 50 || other == 200) && other != line,
                "n then went to line {other} after {line}: {what}"
            );
        }
    }
}

// --- (f) A nested screen, and a different file shown inside the project.

fn reading(s: &Sim) -> String {
    format!(
        "{:?} {:?} {} {}",
        s.app.reading_position_for_test(),
        s.app.preview_search_query(),
        s.app.preview_top_line(),
        s.app.preview_caret_line()
    )
}

#[test]
fn a_preview_comes_back_the_same_after_a_nested_screen_and_after_another_file_in_the_project() {
    let mut body = String::from("# Title\n\n");
    for i in 0..80 {
        body.push_str(&format!("Paragraph {i} with some words.\n\n"));
    }
    for worker in [false, true] {
        let root = repo("prp_nested", &[("a.md", &body), ("b.md", "# B\n\nbee\n")]);
        std::fs::write(root.join("b.md"), "# B\n\nbee changed\n").unwrap();
        let mut s = road(&root, cfg_en(), worker);
        s.select("a.md");
        s.enter();
        settled(&mut s);
        for _ in 0..25 {
            s.key('j');
        }
        s.draw();
        let before = reading(&s);
        // An inner screen pushed over the project's, then popped.
        open_project(&mut s);
        s.tab();
        settle_projects(&mut s);
        s.enter();
        settle_projects(&mut s);
        assert!(
            s.app.project_pushed(),
            "worker={worker}: nothing was pushed:\n{}",
            s.screen()
        );
        s.key('q');
        settle_projects(&mut s);
        assert!(
            s.app.project_active(),
            "worker={worker}: the pop left the project"
        );
        leave_project(&mut s);
        assert_eq!(
            reading(&s),
            before,
            "worker={worker}: after a nested screen"
        );
        // A different file shown from inside the project, then back out of it.
        open_project(&mut s);
        s.enter();
        settle_projects(&mut s);
        wait_diff_target(&mut s, worker);
        settled(&mut s);
        s.see("bee changed");
        s.key('q');
        settle_projects(&mut s);
        assert!(
            s.app.project_active(),
            "worker={worker}: back in the project"
        );
        leave_project(&mut s);
        assert_eq!(reading(&s), before, "worker={worker}: after another file");
        s.see("Paragraph");
    }
}

// --- the pool of a tree filter is the tab's: it keeps up with the files while a screen covers it

/// A tree filter is on, a project screen covers the tree, a file that matches appears meanwhile
/// and the tree is refreshed: after the return the filter finds the new file (the pool was
/// brought up to date while covered, and the restore must not put the older pool back).
#[test]
fn a_tree_filter_pool_made_while_covered_is_not_reverted_by_the_return() {
    for worker in [false, true] {
        for away in BOTH {
            let root = repo(
                "prp_filter_pool",
                &[("xold.txt", "x\n"), ("other.txt", "o\n")],
            );
            let mut s = road(&root, cfg_en(), worker);
            s.key('/');
            for c in "x".chars() {
                s.key(c);
            }
            s.enter();
            settled(&mut s);
            assert!(s.app.filter_query().is_some(), "the filter is on");
            // A preview of a file of the result: the screen that covers it is put back through
            // the tab restore (a tree is put back by `back_to_tree`, which has no saved tab).
            s.select("xold.txt");
            s.enter();
            settled(&mut s);
            assert_eq!(s.app.tab.mode, Mode::Preview, "{away:?} worker={worker}");
            go_away(&mut s, away);
            std::fs::write(root.join("xnew.txt"), "n\n").unwrap();
            s.app.refresh_fs(true).unwrap();
            settled(&mut s);
            come_back(&mut s, away);
            let pool = s.app.filter_pool_names_for_test();
            if matches!(away, Away::ProjectScreen) {
                // Read before anything has run since the return: the pool is the tab's own, brought
                // up to date while covered, not the older one saved at the cover.
                assert!(
                    pool.iter().any(|n| n == "xnew.txt"),
                    "{away:?} worker={worker}: the pool was reverted: {pool:?}"
                );
            }
            settled(&mut s);
            let names: Vec<String> = s
                .app
                .tab
                .entries
                .iter()
                .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            assert!(
                names.iter().any(|n| n == "xnew.txt"),
                "{away:?} worker={worker}: the filter does not find the file made while covered: {names:?}\n{}",
                s.screen()
            );
        }
    }
}

/// The pool is scanned on a worker when the loader is attached (the run loop does). A scan that
/// lands while the screen covers the tab is the tab's pool; the return starts a scan of its own,
/// and until that one lands (nothing applies it here) the pool the tab shows is the one that was
/// delivered while covered, not the older one saved at the cover.
#[test]
fn a_filter_pool_delivered_while_covered_is_still_the_pool_right_after_the_return() {
    for worker in [false, true] {
        let root = repo(
            "prp_filter_pool_async",
            &[("xold.txt", "x\n"), ("other.txt", "o\n")],
        );
        let mut s = road(&root, cfg_en(), worker);
        s.key('/');
        s.key('x');
        s.enter();
        settled(&mut s);
        assert!(s.app.filter_query().is_some(), "the filter is on");
        s.select("xold.txt");
        s.enter();
        settled(&mut s);
        assert_eq!(s.app.tab.mode, Mode::Preview, "worker={worker}");
        let (tx, rx) = std::sync::mpsc::channel();
        s.app.attach_filter_pool_loader(tx);
        open_project(&mut s);
        std::fs::write(root.join("xnew.txt"), "n\n").unwrap();
        s.app.refresh_fs(true).unwrap();
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the scan started while covered lands");
        assert!(s.app.apply_filter_pool(res), "worker={worker}");
        let live = s.app.filter_pool_names_for_test();
        assert!(
            live.iter().any(|n| n == "xnew.txt"),
            "worker={worker}: the scan did not reach the pool: {live:?}"
        );
        leave_project(&mut s);
        let pool = s.app.filter_pool_names_for_test();
        assert!(
            pool.iter().any(|n| n == "xnew.txt"),
            "worker={worker}: the pool delivered while covered was reverted: {pool:?}"
        );
    }
}

// --- One load of one preview: the state that belongs to the load follows the load.

/// A document whose line N (1-based) reads `Line N marker-N`, so what is drawn at a line is
/// readable on the screen.
fn numbered_doc() -> String {
    let mut doc = String::from("# Title\n");
    for i in 2..=90 {
        doc.push_str(&format!("Line {i} marker-{i}\n"));
    }
    doc
}

/// (1) A Markdown diff in the representation it opens in (no `R`), scrolled after its first
/// landing: the return reads the diff again but does not land on the first change again.
#[test]
fn a_default_markdown_diff_keeps_its_scroll_after_the_return() {
    let (mut original, mut changed) = (String::from("# Title\n\n"), String::from("# Title\n\n"));
    for i in 0..80 {
        original.push_str(&format!(
            "Filler paragraph number {i} to take up space.\n\n"
        ));
        changed.push_str(&format!("Filler paragraph number {i} was edited here.\n\n"));
    }
    for worker in [false, true] {
        for away in BOTH {
            let root = repo("prp_md_diff_default", &[("doc.md", &original)]);
            std::fs::write(root.join("doc.md"), &changed).unwrap();
            let mut s = road(&root, cfg_en(), worker);
            s.select("doc.md");
            s.key('d');
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            for _ in 0..12 {
                s.key('j');
            }
            s.draw();
            let before = (
                s.app.diff_view_for_test(),
                s.app.tab.preview_scroll,
                s.app.preview_top_line(),
                s.app.preview_caret_line(),
            );
            assert_eq!(
                before.0,
                DiffView::Rendered,
                "{away:?} worker={worker}: not the formatted diff"
            );
            assert!(
                before.1 > 0 || before.2 > 0 || before.3 > 0,
                "{away:?} worker={worker}: the diff did not scroll: {before:?}"
            );
            s.see("edited here");
            go_away(&mut s, away);
            // The file changes while covered: the return reads it again.
            std::fs::write(
                root.join("doc.md"),
                changed.replace("was edited", "was REVISED"),
            )
            .unwrap();
            come_back(&mut s, away);
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            let after = (
                s.app.diff_view_for_test(),
                s.app.tab.preview_scroll,
                s.app.preview_top_line(),
                s.app.preview_caret_line(),
            );
            assert_eq!(after, before, "{away:?} worker={worker}:\n{}", s.screen());
            // The current content is drawn, at the scroll the reader left (not on the first
            // change again).
            s.see("REVISED here");
            s.dont_see("edited here");
        }
    }
}

/// (2) `doc.md:40` asked while the kind is not decided; the kind and the body arrive while the
/// screen is covered. The return shows the document at line 40, not a provisional kind, and does
/// not land a second time.
#[test]
fn a_line_landed_and_a_kind_resolved_while_covered_stay_resolved_after_the_return() {
    for away in BOTH {
        let root = repo(
            "prp_kind_while_covered",
            &[("doc.md", &numbered_doc()), ("x.txt", "x\n")],
        );
        let path = root.join("doc.md");
        let mut s = road(&root, cfg_en(), true);
        s.app.paste_jump_to(&path, Some(40));
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
                assert!(s.app.project_active(), "{}", s.screen());
            }
            Away::Tab => s.key('t'),
        }
        // Kind and body arrive while covered.
        if matches!(away, Away::ProjectScreen) {
            settled(&mut s);
        }
        match away {
            Away::ProjectScreen => leave_project(&mut s),
            Away::Tab => {
                s.key('[');
                settled(&mut s);
            }
        }
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert!(!what.contains("can not preview"), "{what}");
        assert!(s.app.document_error().is_none(), "{what}");
        assert!(!s.app.has_pending_landing_for_test(), "{what}");
        // The landing on a line shows the document as it is (the Source representation) with the
        // caret on the line: the same on both roads.
        assert!(
            s.app.is_raw_source(),
            "not the Source representation: {what}"
        );
        assert_eq!(s.app.preview_caret_line(), 39, "caret: {what}");
        assert!(
            s.find_text("marker-40").is_some(),
            "line 40 is not on the drawn screen: {what}"
        );
        let pos = s.app.reading_position_for_test();
        s.draw();
        settled(&mut s);
        assert_eq!(
            s.app.reading_position_for_test(),
            pos,
            "landed again: {what}"
        );
    }
}

/// (3) A diff Preview waiting for its body is covered, the load fails while covered, the file is
/// readable again: no DiffPreview landing is revived on the return.
#[test]
fn a_diff_preview_landing_that_failed_while_covered_is_not_revived() {
    use std::os::unix::fs::PermissionsExt;
    let root = repo(
        "prp_diff_failed_covered",
        &[("doc.md", &long_doc("", "ORIGINAL MARKER paragraph."))],
    );
    std::fs::write(
        root.join("doc.md"),
        long_doc("", "CHANGED MARKER paragraph."),
    )
    .unwrap();
    let path = root.join("doc.md");
    let mut s = road(&root, cfg_en(), true);
    s.select("doc.md");
    s.key('d');
    wait_diff_target(&mut s, true);
    settled(&mut s);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    s.key('R');
    assert!(
        s.app.diff_landing_state_for_test().0,
        "the landing is not pending:\n{}",
        s.screen()
    );
    // The state before the cover: the landing waits for the worker's plan of the file (the kind
    // is not resolved yet). A document's text arrives synchronously once the kind is applied (a
    // readable file lands at once), so "kind resolved, text awaited" has no stop of its own here;
    // the failure then comes with the kind's reply, which is what arrives while covered.
    assert_eq!(
        s.app.pending_kind_known_for_test(),
        Some(false),
        "the landing is not waiting for the kind:\n{}",
        s.screen()
    );
    assert!(s.app.document_error().is_none(), "{}", s.screen());
    s.key('p');
    settle_projects_leaving_media(&mut s);
    // The failure arrives while covered.
    settled(&mut s);
    assert!(
        s.app.document_error().is_some(),
        "the body load did not fail while covered:\n{}",
        s.screen()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    leave_project(&mut s);
    settled(&mut s);
    let what = s.screen();
    assert!(
        !s.app.diff_landing_state_for_test().0,
        "a DiffPreview landing was revived:\n{what}"
    );
    // A revived landing would have run on the return and moved to the change at the end.
    assert_eq!(s.app.reading_position_for_test().0, 0, "scrolled: {what}");
    assert!(s.find_text("CHANGED MARKER").is_none(), "landed: {what}");
}

/// (5) The identity names the user's open: the diff's own answer that moves it to the Preview
/// representation is progress of the same open and keeps it. Covered before that answer, the
/// answer and the failure of its body load happen while covered; the file is readable again on the
/// return: the working tab still holds the load the copy was saved in (same identity), so nothing
/// of the old preview is brought back and no landing is revived.
#[test]
fn the_diff_answer_choosing_preview_while_covered_keeps_the_open_identity() {
    use std::os::unix::fs::PermissionsExt;
    let root = repo(
        "prp_diff_preview_identity",
        &[("doc.md", &long_doc("", "ORIGINAL MARKER paragraph."))],
    );
    std::fs::write(
        root.join("doc.md"),
        long_doc("", "CHANGED MARKER paragraph."),
    )
    .unwrap();
    let path = root.join("doc.md");
    let mut cfg = cfg_en();
    cfg.ui.diff_view = "preview".into();
    let mut s = road(&root, cfg, true);
    s.select("doc.md");
    s.key('d');
    assert!(
        !s.app.diff_target_resolved_for_test(),
        "the target was answered before the cover:\n{}",
        s.screen()
    );
    let open_id = s.app.load_id_for_test();
    assert_ne!(open_id, 0);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    s.key('p');
    settle_projects_leaving_media(&mut s);
    // The answer arrives while covered, moves the diff to the Preview representation, and the
    // body of that preview cannot be read.
    wait_diff_target(&mut s, true);
    settled(&mut s);
    assert_eq!(
        s.app.load_id_for_test(),
        open_id,
        "the answer renewed the identity of the open:\n{}",
        s.screen()
    );
    assert!(
        s.app.document_error().is_some(),
        "the body load did not fail while covered:\n{}",
        s.screen()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    leave_project(&mut s);
    settled(&mut s);
    let what = s.screen();
    assert_eq!(s.app.load_id_for_test(), open_id, "identity: {what}");
    assert!(
        !s.app.diff_landing_state_for_test().0,
        "a DiffPreview landing was revived:\n{what}"
    );
    assert_eq!(s.app.reading_position_for_test().0, 0, "scrolled: {what}");
    assert!(s.find_text("CHANGED MARKER").is_none(), "landed: {what}");
}

fn apple_rows(rows: &[u32]) -> String {
    let mut out = format!(r#"<row r="1">{}</row>"#, x_str("A1", "head"));
    for r in rows {
        out += &format!(
            r#"<row r="{r}">{}</row>"#,
            x_str(&format!("A{r}"), &format!("apple {r}"))
        );
    }
    out
}

/// A repository with a sheet (untracked) and a changed Markdown file to show in the project.
fn sheet_repo(name: &str, rows: &[u32]) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = repo(name, &[("b.md", "# B\n\nbee\n")]);
    std::fs::write(root.join("b.md"), "# B\n\nbee changed\n").unwrap();
    let book = root.join("t.xlsx");
    build_xlsx(&book, &[("S", "visible", &apple_rows(rows), "")]);
    (root, book)
}

fn search_on_loading_sheet(root: &std::path::Path) -> Sim {
    let mut s = road(root, cfg_en(), false);
    s.select("t.xlsx");
    s.enter();
    assert!(s.app.is_sheet_loading(), "{}", s.screen());
    s.key('/');
    s.keys("apple");
    s.enter();
    assert!(s.app.search_waits_for_table_for_test(), "{}", s.screen());
    s
}

/// (4a) A search confirmed while the table loads waits for it. Covered, with another file shown
/// meanwhile (the project screen) or in another tab, the request is still there on the return and
/// lands on the first hit when the table arrives.
#[test]
fn a_search_waiting_for_a_table_survives_another_preview_while_covered() {
    for away in BOTH {
        let (root, _) = sheet_repo("prp_search_wait", &[5, 40]);
        let mut s = search_on_loading_sheet(&root);
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
                // Another file's preview is made on the tab under the screen.
                s.app.paste_jump_to(&root.join("b.md"), None);
                s.key('q');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => {
                s.key('t');
                settled(&mut s);
                s.select("b.md");
                s.enter();
                settled(&mut s);
                s.key('[');
                settle_projects_leaving_media(&mut s);
            }
        }
        let what = format!("{away:?}\n{}", s.screen());
        assert!(!s.app.project_active(), "{what}");
        assert_eq!(s.app.preview_search_query(), Some("apple"), "{what}");
        assert!(
            s.app.search_waits_for_table_for_test(),
            "request lost: {what}"
        );
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert_eq!(s.app.search_status(), Some((1, 2)), "{what}");
        assert_eq!(s.app.table_cursor(), (4, 0), "first hit: {what}");
        assert!(!s.app.search_waits_for_table_for_test(), "{what}");
    }
}

/// (4b) The request consumed while covered (the same load: the table arrived under the screen)
/// moved the cursor to the first hit there: the return keeps that cursor and does not make the
/// request again. The tab switch (the answer is dropped, the request is made after the return)
/// gives the same cursor.
#[test]
fn a_search_request_consumed_while_covered_is_not_made_again() {
    for away in BOTH {
        let (root, _) = sheet_repo("prp_search_consumed", &[5, 40]);
        let mut s = search_on_loading_sheet(&root);
        assert_eq!(s.app.table_cursor(), (0, 0), "{away:?}");
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
                // The table arrives under the project screen: the request is consumed there and
                // the cursor moved to the first hit.
                settled(&mut s);
                assert!(!s.app.search_waits_for_table_for_test(), "{}", s.screen());
                assert_eq!(s.app.table_cursor(), (4, 0), "{}", s.screen());
                s.key('q');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => {
                s.key('t');
                settled(&mut s);
                s.key('[');
                settle_projects_leaving_media(&mut s);
            }
        }
        let what = format!("{away:?}\n{}", s.screen());
        assert!(!s.app.project_active(), "{what}");
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert!(
            !s.app.search_waits_for_table_for_test(),
            "the request was made again: {what}"
        );
        assert_eq!(s.app.table_cursor(), (4, 0), "first hit: {what}");
        assert_eq!(s.app.search_status(), Some((1, 2)), "{what}");
    }
}

/// (5) The hit that was current is gone while covered; the restored caret is between a near
/// earlier hit and a far later one. The re-derivation makes the nearest hit current (a tie goes to
/// the earlier one), and `n`/`N` move on from it.
#[test]
fn the_search_after_the_return_is_current_on_the_hit_nearest_the_restored_caret() {
    for away in BOTH {
        let root = repo("prp_search_nearest", &[("x.txt", "x\n")]);
        let book = root.join("t.xlsx");
        let write = |rows: &[u32], k: u64| {
            build_xlsx(&book, &[("S", "visible", &apple_rows(rows), "")]);
            let f = std::fs::OpenOptions::new().write(true).open(&book).unwrap();
            f.set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000 + k),
            )
            .unwrap();
        };
        write(&[12, 60], 0);
        let mut s = road(&root, cfg_en(), false);
        s.select("t.xlsx");
        s.enter();
        settled(&mut s);
        s.key('/');
        s.keys("apple");
        s.enter();
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((1, 2)), "{away:?}");
        assert_eq!(s.app.table_cursor(), (11, 0), "{away:?}");
        go_away(&mut s, away);
        write(&[10, 60], 1);
        come_back(&mut s, away);
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert_eq!(s.app.table_cursor(), (11, 0), "{what}");
        assert!(s.app.table_cell_is_hit(9, 0), "{what}");
        assert_eq!(
            s.app.search_status(),
            Some((1, 2)),
            "the nearer hit: {what}"
        );
        s.key('n');
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((2, 2)), "{what}");
        assert_eq!(s.app.table_cursor(), (59, 0), "{what}");
        s.key('N');
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((1, 2)), "{what}");
        assert_eq!(s.app.table_cursor(), (9, 0), "{what}");
    }
}

/// (5b) The same as (5) for a table that is already here when the search is found again (a CSV is
/// read in the app, not on a sheet worker, so `sheet_load_in_flight()` is false and
/// `rescan_stale_search` itself picks the hit; `rescan_table_search` of a sheet that lands is not
/// the code that runs).
#[test]
fn the_search_of_a_table_that_is_here_is_current_on_the_hit_nearest_the_restored_caret() {
    for away in BOTH {
        let root = repo("prp_search_nearest_csv", &[("x.txt", "x\n")]);
        let book = root.join("t.csv");
        let write = |hits: &[u32]| {
            let mut text = String::from("name,qty\n");
            for r in 2..=80u32 {
                let word = if hits.contains(&r) { "apple" } else { "pear" };
                text.push_str(&format!("{word} {r},{r}\n"));
            }
            std::fs::write(&book, text).unwrap();
        };
        write(&[12, 60]);
        let mut s = road(&root, cfg_en(), false);
        s.select("t.csv");
        s.enter();
        settled(&mut s);
        assert!(s.app.is_table_preview(), "{away:?}\n{}", s.screen());
        assert!(!s.app.is_sheet_loading(), "{away:?}");
        s.key('/');
        s.keys("apple");
        s.enter();
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((1, 2)), "{away:?}");
        let caret = s.app.table_cursor();
        go_away(&mut s, away);
        write(&[10, 60]);
        come_back(&mut s, away);
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert_eq!(s.app.table_cursor(), caret, "{what}");
        assert_eq!(
            s.app.search_status(),
            Some((1, 2)),
            "the nearer hit: {what}"
        );
        s.key('n');
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((2, 2)), "{what}");
        s.key('N');
        settled(&mut s);
        assert_eq!(s.app.search_status(), Some((1, 2)), "{what}");
    }
}
