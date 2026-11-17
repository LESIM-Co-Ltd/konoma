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

/// Moves the project screen's cursor (`j`) onto the row of `target`.
#[track_caller]
fn project_cursor_to(s: &mut Sim, target: &std::path::Path, what: &str) {
    for _ in 0..8 {
        if s.app.project_row_target().as_deref() == Some(target) {
            break;
        }
        s.key('j');
        settle_projects(s);
    }
    assert_eq!(
        s.app.project_row_target().as_deref(),
        Some(target),
        "{what}: the project screen does not list {}\n{}",
        target.display(),
        s.screen()
    );
}

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

/// (3) A diff Preview waiting for its body is left (a project screen covers it, or another tab is
/// shown), the file cannot be read meanwhile and is readable again on the return. Covering is
/// leaving the tab: the load goes with the screen, so its failure never reaches the tab, and the
/// return reads the file again. The landing the tab still holds then runs on the readable file,
/// on both roads alike: on the first change, as in (i).
#[test]
fn a_diff_preview_landing_left_while_its_file_cannot_be_read_lands_on_the_return() {
    use std::os::unix::fs::PermissionsExt;
    for away in BOTH {
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
            "{away:?}: the landing is not pending:\n{}",
            s.screen()
        );
        // The state before leaving: the landing waits for the worker's plan of the file (the kind
        // is not resolved yet).
        assert_eq!(
            s.app.pending_kind_known_for_test(),
            Some(false),
            "{away:?}: the landing is not waiting for the kind:\n{}",
            s.screen()
        );
        assert!(s.app.document_error().is_none(), "{away:?}: {}", s.screen());
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => s.key('t'),
        }
        // Whatever the worker answers meanwhile is not the tab's any more.
        settled(&mut s);
        assert!(
            s.app.document_error().is_none(),
            "{away:?}: the load of the screen that was left failed in front:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match away {
            Away::ProjectScreen => leave_project(&mut s),
            Away::Tab => s.key('['),
        }
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert!(s.app.document_error().is_none(), "still failed: {what}");
        assert!(
            !s.app.has_pending_landing_for_test(),
            "a landing is pending: {what}"
        );
        assert!(
            s.find_text("CHANGED MARKER").is_some(),
            "not on the first change: {what}"
        );
    }
}

/// (3c) The same waiting state with the screen in front all along: the read of the body fails
/// while the landing waits for it. The failure ends the landing (`document_failed`), so when the
/// file is readable again a later read shows it where the reader was and does not land a
/// position asked of the failed document.
#[test]
fn a_diff_preview_landing_whose_body_failed_in_front_is_not_revived() {
    use crate::app::HoldOpens;
    use crate::plugins::test_plugins::Latch;
    use std::os::unix::fs::PermissionsExt;
    let root = repo(
        "prp_diff_body_failed_front",
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
    let gate = std::sync::Arc::new(Latch::default());
    let hold = HoldOpens::until(&gate);
    s.key('R');
    assert!(s.app.diff_landing_state_for_test().0, "{}", s.screen());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while s.app.pending_kind_known_for_test() == Some(false) {
        let left = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("the kind was never resolved");
        let res = s
            .media_rx
            .as_ref()
            .expect("with_media")
            .recv_timeout(left)
            .expect("the worker answers");
        s.app.apply_media(res);
    }
    assert_eq!(
        s.app.pending_kind_known_for_test(),
        Some(true),
        "{}",
        s.screen()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    gate.open();
    settled(&mut s);
    assert!(
        s.app.document_error().is_some(),
        "the body load did not fail:\n{}",
        s.screen()
    );
    assert!(
        !s.app.has_pending_landing_for_test(),
        "the failure did not end the landing:\n{}",
        s.screen()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(hold);
    s.app.reload_preview();
    settled(&mut s);
    assert!(s.app.document_error().is_none(), "{}", s.screen());
    assert!(
        s.find_text("Title").is_some(),
        "not where the reader was:\n{}",
        s.screen()
    );
    assert!(
        s.find_text("CHANGED MARKER").is_none(),
        "the landing of the failed document was revived:\n{}",
        s.screen()
    );
}

/// (3b) The other waiting state of the same landing: the kind is resolved and only the document's
/// body is awaited (the plugin's `open` is held by the gate). The screen is left there (a project
/// screen covers it, or another tab is shown), the read fails meanwhile, the file is readable
/// again. Covering is leaving the tab, on both roads alike: the held open is the tab's no longer
/// (leaving moves the generation, so its answer is dropped as stale) and the tab never sees the
/// failure; the return opens the file again and the landing it still holds runs on a readable
/// file.
#[test]
fn a_diff_preview_landing_whose_body_failed_after_leaving_lands_on_the_return() {
    use crate::app::HoldOpens;
    use crate::plugins::test_plugins::Latch;
    use std::os::unix::fs::PermissionsExt;
    for away in BOTH {
        let root = repo(
            "prp_diff_body_failed_covered",
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
        // From here every plugin `open` waits for the gate: the body cannot arrive.
        let gate = std::sync::Arc::new(Latch::default());
        let hold = HoldOpens::until(&gate);
        s.key('R');
        assert!(
            s.app.diff_landing_state_for_test().0,
            "{away:?}: the landing is not pending:\n{}",
            s.screen()
        );
        // The worker's plan of the file resolves the kind; the document's open is then held.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while s.app.pending_kind_known_for_test() == Some(false) {
            let left = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("the kind was never resolved");
            let res = s
                .media_rx
                .as_ref()
                .expect("with_media")
                .recv_timeout(left)
                .expect("the worker answers");
            s.app.apply_media(res);
        }
        assert_eq!(
            s.app.pending_kind_known_for_test(),
            Some(true),
            "{away:?}: the landing is not waiting for the body with the kind resolved:\n{}",
            s.screen()
        );
        assert!(
            s.app.diff_landing_state_for_test().0,
            "{away:?}: not a DiffPreview landing:\n{}",
            s.screen()
        );
        assert!(
            s.app.document_text_missing() && s.app.document_error().is_none(),
            "{away:?}: the body must still be on its way:\n{}",
            s.screen()
        );
        // The file cannot be read when the held open goes on.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
                assert!(s.app.project_active(), "{}", s.screen());
            }
            Away::Tab => s.key('t'),
        }
        gate.open();
        settled(&mut s);
        // The failure of the open that was left does not reach the screen in front.
        assert!(
            s.app.document_error().is_none(),
            "{away:?}: the failure of the left open reached the screen in front:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match away {
            Away::ProjectScreen => leave_project(&mut s),
            Away::Tab => s.key('['),
        }
        settled(&mut s);
        drop(hold);
        let what = format!("{away:?}\n{}", s.screen());
        assert!(
            !s.app.has_pending_landing_for_test(),
            "a landing is pending: {what}"
        );
        assert!(s.app.document_error().is_none(), "still failed: {what}");
        // The landing the tab held ran on the readable file: on the first change.
        assert!(
            s.find_text("CHANGED MARKER").is_some(),
            "not landed: {what}"
        );
    }
}

/// (5) The identity names the user's open: the diff's own answer that moves it to the Preview
/// representation is progress of the same open and keeps it. The screen is left before that
/// answer (a project screen covers it, or another tab is shown) while the file cannot be read.
/// Covering is leaving the tab, on both roads alike: the answer asked for the screen that was left
/// is not the tab's any more, so nothing of it (a Preview representation, a failed body) happens
/// behind the screen in front. The return asks again on the readable file: the same open
/// (identity) gets its first answer there and lands on the first change.
#[test]
fn the_diff_answer_choosing_preview_after_leaving_keeps_the_open_identity() {
    use std::os::unix::fs::PermissionsExt;
    for away in BOTH {
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
            "{away:?}: the target was answered before the cover:\n{}",
            s.screen()
        );
        let open_id = s.app.load_id_for_test();
        assert_ne!(open_id, 0);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => s.key('t'),
        }
        settled(&mut s);
        assert!(
            !s.app.diff_target_resolved_for_test() && s.app.document_error().is_none(),
            "{away:?}: the answer of the screen that was left landed in front:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match away {
            Away::ProjectScreen => leave_project(&mut s),
            Away::Tab => s.key('['),
        }
        wait_diff_target(&mut s, true);
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert_eq!(s.app.load_id_for_test(), open_id, "identity: {what}");
        assert!(s.app.document_error().is_none(), "failed: {what}");
        assert!(
            !s.app.has_pending_landing_for_test(),
            "a landing is pending: {what}"
        );
        assert!(
            s.find_text("CHANGED MARKER").is_some(),
            "not on the first change: {what}"
        );
    }
}

/// Takes the answer of the diff target resolution out of the mailbox without applying it (the
/// other results that arrive first are applied, as the main loop applies them).
fn take_diff_target_answer(s: &mut Sim) -> crate::app::MediaResult {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let left = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("the diff target was never answered");
        let res = s
            .media_rx
            .as_ref()
            .expect("with_media")
            .recv_timeout(left)
            .expect("the worker answers");
        if res.is_diff_target_answer_for_test() {
            return res;
        }
        s.app.apply_media(res);
    }
}

fn small_diff_repo(name: &str) -> std::path::PathBuf {
    let root = repo(name, &[("doc.md", "# Title\n\nORIGINAL paragraph.\n")]);
    std::fs::write(root.join("doc.md"), "# Title\n\nCHANGED paragraph.\n").unwrap();
    root
}

/// The answer of the diff target resolution that was asked before the screen was left reaches the
/// main loop while something else is in front (a project screen covers the diff, or another tab
/// is shown). Covering is leaving the tab, on both roads alike: the answer is not applied to the
/// screen that was left (no representation is chosen and nothing is read for it behind the screen
/// in front), and the return asks again.
#[test]
fn a_diff_target_answer_asked_before_leaving_does_not_land_behind_the_screen_in_front() {
    for away in BOTH {
        let root = small_diff_repo("prp_diff_answer_left");
        let mut s = road(&root, cfg_en(), true);
        s.select("doc.md");
        s.key('d');
        assert!(
            !s.app.diff_target_resolved_for_test(),
            "{away:?}: the target was answered before the screen was left"
        );
        let answer = take_diff_target_answer(&mut s);
        go_away(&mut s, away);
        let (applied, views) =
            crate::test_support::count_apply_diff_view_calls(|| s.app.apply_media(answer));
        assert!(
            !applied && views == 0 && !s.app.diff_target_resolved_for_test(),
            "{away:?}: the answer landed behind the screen in front (applied={applied}, \
             apply_diff_view ran {views} times):\n{}",
            s.screen()
        );
        come_back(&mut s, away);
        wait_diff_target(&mut s, true);
        settled(&mut s);
        assert!(
            s.find_text("CHANGED paragraph").is_some(),
            "{away:?}: the diff is not shown after the return:\n{}",
            s.screen()
        );
    }
}

/// A watch event while the diff's screen is left (a project screen covers it, or another tab is
/// shown) asks no diff target resolution for that screen: nothing targets it until it is in front
/// again, and the return reads the file as it is then. In front, the same event asks (the count
/// is live).
#[test]
fn a_watch_event_while_the_diff_is_left_asks_no_resolution_for_it() {
    for away in BOTH {
        let root = small_diff_repo("prp_diff_watch_left");
        let path = root.join("doc.md");
        let mut s = road(&root, cfg_en(), true);
        s.select("doc.md");
        s.key('d');
        wait_diff_target(&mut s, true);
        settled(&mut s);
        let ((), in_front) = crate::test_support::count_diff_resolution_asks(|| {
            s.app.refresh_fs_watched(false, std::slice::from_ref(&path))
        });
        assert!(
            in_front >= 1,
            "{away:?}: a watch event in front asks again ({in_front})"
        );
        wait_diff_target(&mut s, true);
        settled(&mut s);
        go_away(&mut s, away);
        std::fs::write(&path, "# Title\n\nEDITED WHILE LEFT paragraph.\n").unwrap();
        let ((), left) = crate::test_support::count_diff_resolution_asks(|| {
            s.app.refresh_fs_watched(false, std::slice::from_ref(&path))
        });
        assert_eq!(
            left,
            0,
            "{away:?}: the watch event asked a resolution for the screen that was left:\n{}",
            s.screen()
        );
        come_back(&mut s, away);
        wait_diff_target(&mut s, true);
        settled(&mut s);
        assert!(
            s.find_text("EDITED WHILE LEFT").is_some(),
            "{away:?}: the return does not read the file as it is now:\n{}",
            s.screen()
        );
    }
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
                // Another file's preview is made on the tab under the screen by the project
                // screen's own open (Enter on b.md's row, then `q` back to the screen): a paste
                // jump would leave the project screens first and land on the base screen. The
                // working tab then holds another load (b.md's project diff) when the screen is
                // left.
                let b = root.join("b.md");
                project_cursor_to(&mut s, &b, &format!("{away:?}"));
                s.enter();
                settle_projects_leaving_media(&mut s);
                assert_eq!(
                    s.app.tab.preview_path.as_deref(),
                    Some(b.as_path()),
                    "{away:?}: b.md was not opened under the cover\n{}",
                    s.screen()
                );
                s.key('q');
                settle_projects_leaving_media(&mut s);
                assert!(
                    s.app.project_active(),
                    "{away:?}: `q` did not go back to the project screen\n{}",
                    s.screen()
                );
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

/// (4b) A search confirmed while the table loads, then the screen is left before the table
/// arrives (a project screen covers it, or another tab is shown). Covering is leaving the tab: the
/// load goes with the screen and the table never arrives behind it, on both roads alike. The
/// return loads the table again and the request is consumed there once: the cursor is on the
/// first hit and nothing waits any more.
#[test]
fn a_search_request_left_with_its_loading_table_is_consumed_once_on_the_return() {
    for away in BOTH {
        let (root, _) = sheet_repo("prp_search_consumed", &[5, 40]);
        let mut s = search_on_loading_sheet(&root);
        assert_eq!(s.app.table_cursor(), (0, 0), "{away:?}");
        match away {
            Away::ProjectScreen => {
                s.key('p');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => s.key('t'),
        }
        // Nothing of the table is loaded behind the screen in front.
        settled(&mut s);
        assert!(
            s.find_text("apple 5").is_none() && !s.app.is_sheet_loading(),
            "{away:?}: the table was loaded behind the screen in front:\n{}",
            s.screen()
        );
        match away {
            Away::ProjectScreen => {
                s.key('q');
                settle_projects_leaving_media(&mut s);
            }
            Away::Tab => {
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
            "the request still waits after the table arrived: {what}"
        );
        assert_eq!(s.app.table_cursor(), (4, 0), "first hit: {what}");
        assert_eq!(s.app.search_status(), Some((1, 2)), "{what}");
        assert!(s.find_text("apple 5").is_some(), "{what}");
        // A later refresh of the same table does not make the request again.
        s.key('j');
        s.app.reload_preview();
        settled(&mut s);
        let what = format!("{away:?}\n{}", s.screen());
        assert!(!s.app.search_waits_for_table_for_test(), "{what}");
        assert_eq!(
            s.app.table_cursor(),
            (5, 0),
            "the request was made again: {what}"
        );
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

// --- The `C` list's pending cursor identity goes with the tree it belongs to.

/// Changed c1, c2, c3 and an unchanged k.txt, committed: `C` lists the three changed files.
fn changed_files_repo(name: &str) -> std::path::PathBuf {
    let root = repo(
        name,
        &[
            ("c1.txt", "c\n"),
            ("c2.txt", "c\n"),
            ("c3.txt", "c\n"),
            ("k.txt", "c\n"),
        ],
    );
    for n in ["c1.txt", "c2.txt", "c3.txt"] {
        std::fs::write(root.join(n), "changed\n").unwrap();
    }
    root
}

/// The file the tree cursor is on.
fn cursor_file(s: &Sim) -> Option<String> {
    s.app
        .tab
        .entries
        .get(s.app.tab.selected)
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
}

fn tree_names(s: &Sim) -> Vec<String> {
    s.app
        .tab
        .entries
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Lands every status scan that arrives, until none comes for a while.
fn land_scans(s: &mut Sim, rx: &std::sync::mpsc::Receiver<crate::app::StatusResult>) -> usize {
    let mut n = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        s.app.apply_statuses(res);
        n += 1;
    }
    s.draw();
    n
}

/// What is done while the tab is away, after the scan of the deferred `C` rebuild has landed.
#[derive(Clone, Copy, Debug)]
enum Meanwhile {
    /// Nothing: the working tab still holds the preview load the tab was saved with.
    Nothing,
    /// Another file opened (Enter on the project screen, or on the other tab's tree), then `q`.
    OpenFile,
    /// A pasted path to the unchanged k.txt jumped to (a preview of another file). Over a project
    /// screen the jump leaves the screens first (`pasted_jump_leaves_the_project_screens`).
    PasteJump,
}

/// The `C` list with the cursor on c3, the tree drawn with that row reversed.
#[track_caller]
fn assert_cursor_on_c3(s: &Sim, what: &str) {
    assert_eq!(
        tree_names(s),
        ["aaa_new.txt", "c1.txt", "c2.txt", "c3.txt"],
        "{what}: not the changed list\n{}",
        s.screen()
    );
    assert_eq!(
        cursor_file(s).as_deref(),
        Some("c3.txt"),
        "{what}: the cursor left its file\n{}",
        s.screen()
    );
}

#[track_caller]
fn assert_c3_drawn_under_the_cursor(s: &Sim, what: &str) {
    let reversed = |name: &str| {
        s.style_of(name)
            .is_some_and(|st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED))
    };
    assert!(
        reversed("c3.txt") && !reversed("c2.txt") && !reversed("c1.txt"),
        "{what}: the drawn cursor is not on c3.txt\n{}",
        s.screen()
    );
}

/// `C` on, cursor on c3 (real keys); a new file makes the next `C` rebuild wait for a status scan
/// (the real loader), so the list is the whole tree and the cursor identity waits with it; Enter
/// opens a preview, the tab goes away (`p`, or a new tab), the scan lands while it is away,
/// `meanwhile` is done, the tab comes back (`q`, or `[`). The return puts the tree back from the
/// saved copy with the identity it waits with, and the scan that follows lands the list on c3.
fn changed_anchor_road(name: &str, away: Away, meanwhile: Meanwhile) {
    let what = format!("{away:?} {meanwhile:?}");
    let root = changed_files_repo(name);
    let mut s = road(&root, cfg_en(), false);
    s.key('C');
    settled(&mut s);
    assert_eq!(tree_names(&s), ["c1.txt", "c2.txt", "c3.txt"], "{what}");
    for _ in 0..3 {
        if cursor_file(&s).as_deref() == Some("c3.txt") {
            break;
        }
        s.key('j');
    }
    assert_eq!(cursor_file(&s).as_deref(), Some("c3.txt"), "{what}");
    let (tx, rx) = std::sync::mpsc::channel();
    s.app.attach_status_loader(tx);
    let newfile = root.join("aaa_new.txt");
    std::fs::write(&newfile, "y\n").unwrap();
    s.app.refresh_fs_watched(false, &[newfile]);
    // The rebuild waits for the scan: the list is the whole tree meanwhile.
    assert!(
        tree_names(&s).iter().any(|n| n == "k.txt"),
        "{what}: the rebuild did not wait for the scan: {:?}",
        tree_names(&s)
    );
    s.enter();
    settled(&mut s);
    assert_eq!(s.app.tab.mode, Mode::Preview, "{what}\n{}", s.screen());
    let shown = s.app.tab.preview_path.clone();
    go_away(&mut s, away);
    // The scan lands while the tab is away, on both roads alike. Over a project screen the working
    // tab consumes the anchor there (its list is rebuilt behind the screen and thrown away on the
    // return), which is what tells the anchor's group apart: the return puts the saved one back.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the deferred scan lands");
    s.app.apply_statuses(res);
    assert!(
        !s.app.changed_anchor_left_to_status(),
        "{what}: the scan did not consume the anchor of the screen in front"
    );
    match meanwhile {
        Meanwhile::Nothing => {}
        Meanwhile::OpenFile => {
            s.enter();
            settle_projects(&mut s);
            settled(&mut s);
            assert_eq!(s.app.tab.mode, Mode::Preview, "{what}\n{}", s.screen());
            assert_ne!(s.app.tab.preview_path, shown, "{what}: not another file");
            s.key('q');
            settle_projects(&mut s);
            assert_eq!(
                s.app.project_active(),
                matches!(away, Away::ProjectScreen),
                "{what}: `q` did not go back to where the file was opened\n{}",
                s.screen()
            );
        }
        Meanwhile::PasteJump => {
            s.app.paste_jump_to(&root.join("k.txt"), None);
            settled(&mut s);
            assert_eq!(
                s.app.tab.preview_path.as_deref(),
                Some(root.join("k.txt").as_path()),
                "{what}"
            );
            if matches!(away, Away::ProjectScreen) {
                pasted_jump_leaves_the_project_screens(&mut s, &rx, &root, &what);
                return;
            }
        }
    }
    come_back(&mut s, away);
    assert_eq!(s.app.tab.mode, Mode::Preview, "{what}\n{}", s.screen());
    assert_eq!(s.app.tab.preview_path, shown, "{what}");
    // The return's refresh waits for its own scan again (the list is the whole tree until then).
    let landed = land_scans(&mut s, &rx);
    assert!(landed >= 1, "{what}: the return asked for no scan");
    assert_cursor_on_c3(&s, &format!("{what}, after the return's scan"));
    // Back to the tree: the drawn list has the cursor on c3, also once the scan that the tree's own
    // refresh asks for has landed.
    s.key('q');
    settled(&mut s);
    assert_eq!(s.app.tab.mode, Mode::Tree, "{what}");
    land_scans(&mut s, &rx);
    assert_cursor_on_c3(&s, &format!("{what}, in the tree"));
    assert_c3_drawn_under_the_cursor(&s, &what);
}

/// The paste jump to k.txt made while the project screen covers the tab: a jump, like a bookmark
/// jump, leaves the project screens first (`leave_project_screens_for_jump`) and lands on the base
/// screen, so it is not an open under the cover and there is no screen left to come back from.
/// Leaving puts the saved tree back (`put_tab_back`): the whole tree, as it was saved while its
/// `C` rebuild waited for a scan, with the identity (c3) still waiting with it. The scan that
/// landed while covered rebuilt only the working copy's list, which the saved tree replaces, and
/// the return's refresh leaves the rebuild to the scan it asked for (one is in flight). The jump then reveals k.txt in that whole tree, which moves `selected` only, not
/// the waiting identity. So when the scan lands, `apply_statuses` rebuilds the `C` list on the
/// identity it waits with: the changed files with the cursor on c3 (k.txt, unchanged, is not in
/// the list), while the preview stays k.txt's. Back in the tree it is the same.
#[track_caller]
fn pasted_jump_leaves_the_project_screens(
    s: &mut Sim,
    rx: &std::sync::mpsc::Receiver<crate::app::StatusResult>,
    root: &std::path::Path,
    what: &str,
) {
    let k = root.join("k.txt");
    assert!(
        !s.app.project_active() && s.app.tab.project_navigation.is_none(),
        "{what}: the jump did not leave the project screens\n{}",
        s.screen()
    );
    assert_eq!(s.app.tab.mode, Mode::Preview, "{what}\n{}", s.screen());
    assert!(
        tree_names(s).iter().any(|n| n == "k.txt"),
        "{what}: the saved tree (the whole tree waiting for its scan) was not put back: {:?}",
        tree_names(s)
    );
    assert!(
        s.app.changed_anchor_left_to_status() && s.app.git_status_in_flight(),
        "{what}: the `C` rebuild does not wait for the return's scan with its identity"
    );
    assert_eq!(
        cursor_file(s).as_deref(),
        Some("k.txt"),
        "{what}: the jump did not land on k.txt in the tree"
    );
    let landed = land_scans(s, rx);
    assert!(landed >= 1, "{what}: the return asked for no scan");
    assert_eq!(
        s.app.tab.preview_path.as_deref(),
        Some(k.as_path()),
        "{what}: the scan moved the preview off k.txt"
    );
    assert_cursor_on_c3(s, &format!("{what}, after the scan that follows the jump"));
    s.key('q');
    settled(s);
    assert_eq!(s.app.tab.mode, Mode::Tree, "{what}");
    land_scans(s, rx);
    assert_cursor_on_c3(s, &format!("{what}, in the tree"));
    assert_c3_drawn_under_the_cursor(s, what);
}

#[test]
fn a_changed_list_cursor_waiting_for_a_scan_comes_back_on_its_file() {
    for away in BOTH {
        changed_anchor_road("prp_anchor_same", away, Meanwhile::Nothing);
    }
}

#[test]
fn a_changed_list_cursor_waiting_for_a_scan_comes_back_on_its_file_after_another_file() {
    for away in BOTH {
        for meanwhile in [Meanwhile::OpenFile, Meanwhile::PasteJump] {
            changed_anchor_road("prp_anchor_other", away, meanwhile);
        }
    }
}

// --- Which open a preview came from and the baseline of its diff are the preview's.

/// What the restored preview says of the diff it belongs to.
fn diff_origin(
    s: &Sim,
) -> (
    Option<std::path::PathBuf>,
    bool,
    bool,
    konoma_vcs_core::vcs::Preference,
) {
    (
        s.app.tab.preview_path.clone(),
        s.app.preview_from_diff_for_test(),
        s.app.preview_is_diff_representation(),
        s.app.diff_read_context_for_test().backend,
    )
}

/// A.md's diff opened from the project screen (the Git backend's baseline) and moved to its
/// Preview representation (`R`); the tab goes away (`p`, or a new tab) and another preview of
/// b.txt is opened there: under the project screen by its own open (b.txt's project diff: its
/// Source diff, no diff's Preview, then an ordinary preview of c.txt jumped to from it, still
/// under the cover), in the other tab by a paste jump (an ordinary preview, no baseline); the tab
/// comes back (`q`, or `[`). A comes back as the diff's Preview with the Git
/// baseline, and `R` goes back to the Source diff of that baseline, the same on both roads.
#[test]
fn a_diff_preview_comes_back_with_its_origin_and_baseline_after_another_preview() {
    use konoma_vcs_core::vcs::Preference;
    let mut results = Vec::new();
    for away in BOTH {
        let root = repo(
            "prp_diff_origin",
            &[
                ("a.md", "# A\n\nORIGINAL LINE\n"),
                ("b.txt", "plain bee\n"),
                ("c.txt", "plain sea\n"),
            ],
        );
        std::fs::write(root.join("a.md"), "# A\n\nCHANGED LINE\n").unwrap();
        // b.txt is changed too, so the project screen lists it (the open made under the cover).
        std::fs::write(root.join("b.txt"), "plain bee changed\n").unwrap();
        let a = root.join("a.md");
        let mut s = road(&root, cfg_en(), false);
        open_project(&mut s);
        s.enter();
        settle_projects(&mut s);
        settled(&mut s);
        assert!(s.app.is_git_diff_preview(), "{away:?}\n{}", s.screen());
        assert_eq!(
            s.app.tab.preview_path.as_deref(),
            Some(a.as_path()),
            "{away:?}"
        );
        assert_eq!(
            s.app.diff_read_context_for_test().backend,
            Preference::Git,
            "{away:?}: the project's diff has no explicit baseline"
        );
        for _ in 0..3 {
            if s.app.preview_is_diff_representation() {
                break;
            }
            s.key('R');
            settled(&mut s);
        }
        let before = diff_origin(&s);
        assert_eq!(
            before,
            (Some(a.clone()), true, true, Preference::Git),
            "{away:?}: not the diff's Preview\n{}",
            s.screen()
        );
        go_away(&mut s, away);
        let b = root.join("b.txt");
        match away {
            Away::ProjectScreen => {
                // Under the cover the other file is opened by the project screen's own open
                // (Enter on b.txt's row, then `q` back to the screen): a paste jump would leave
                // the project screens first and land on the base screen. That open is b.txt's
                // project diff, not an ordinary preview: its Source diff (a text file has no
                // Preview representation, so it is no diff's Preview) with the project's (Git)
                // baseline.
                project_cursor_to(&mut s, &b, &format!("{away:?}"));
                s.enter();
                settle_projects(&mut s);
                settled(&mut s);
                assert!(s.app.is_git_diff_preview(), "{away:?}\n{}", s.screen());
                assert_eq!(
                    diff_origin(&s),
                    (Some(b.clone()), false, false, Preference::Git),
                    "{away:?}: the file opened meanwhile is not b.txt's project diff\n{}",
                    s.screen()
                );
                // b.txt's diff has A's baseline (Git: both are the git project's opens), so a
                // baseline taken from the working tab would not show. From that preview a paste
                // jump opens c.txt as an ordinary preview (no baseline, `Auto`) and the cover is
                // kept: the project screens are behind the preview opened from them (no project
                // screen is in front for the jump to leave), and `q` goes back to them.
                // The one close rule this follows: `close_preview` (src/lib.rs), back to the
                // project canvas the preview was opened from.
                let c = root.join("c.txt");
                s.app.paste_jump_to(&c, None);
                settled(&mut s);
                assert_eq!(
                    diff_origin(&s),
                    (Some(c.clone()), false, false, Preference::Auto),
                    "{away:?}: the preview jumped to meanwhile is not an ordinary one\n{}",
                    s.screen()
                );
                s.key('q');
                settle_projects(&mut s);
                assert!(
                    s.app.project_active(),
                    "{away:?}: `q` did not go back to the project screen\n{}",
                    s.screen()
                );
            }
            Away::Tab => {
                s.app.paste_jump_to(&b, None);
                settled(&mut s);
                assert_eq!(
                    diff_origin(&s),
                    (Some(b.clone()), false, false, Preference::Auto),
                    "{away:?}: the preview opened meanwhile is not an ordinary one\n{}",
                    s.screen()
                );
            }
        }
        come_back(&mut s, away);
        settled(&mut s);
        let back = diff_origin(&s);
        assert_eq!(back, before, "{away:?}: after the return\n{}", s.screen());
        s.see("CHANGED LINE");
        // `R` from the diff's Preview goes back to its Source diff, with the same baseline.
        s.key('R');
        settled(&mut s);
        let source = (
            s.app.is_git_diff_preview(),
            s.app.diff_view_for_test(),
            s.app.diff_read_context_for_test().backend,
        );
        assert_eq!(
            source,
            (true, crate::app::DiffView::Source, Preference::Git),
            "{away:?}: `R` did not go back to the Source diff\n{}",
            s.screen()
        );
        s.see("ORIGINAL LINE");
        s.see("CHANGED LINE");
        results.push((back.1, back.2, back.3, source));
    }
    assert_eq!(results[0], results[1], "the two roads differ");
}
