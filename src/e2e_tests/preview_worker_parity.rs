//! A file opened as a preview must come out the same whether its kind is decided on the spot (an
//! `App` without the preview resolution worker, as the tests build it) or on the worker (as `main`
//! attaches it). The two roads share `App::apply_resolved_preview`; these tests pin that every
//! file a standard plugin takes is opened and drawn the same on both.

use super::*;

/// Every file under `samples/`, directories walked (`code/` holds files of its own).
fn sample_files() -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("samples dir") {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples"),
        &mut files,
    );
    files.sort();
    files
}

/// The drawn rows of `s`, a wide symbol counted once (the cell behind it keeps whatever an
/// earlier frame left there, which is not what is drawn).
fn rows(s: &Sim) -> Vec<String> {
    super::table_parity::buffer_rows(s.term.backend().buffer())
}

/// Every drawn cell with its colours and modifiers (a half-block picture is drawn in colour, not
/// in characters), a wide symbol counted once like `rows`.
type Cell = (
    String,
    ratatui::style::Color,
    ratatui::style::Color,
    ratatui::style::Modifier,
);
fn cells(s: &Sim) -> Vec<Vec<Cell>> {
    let buf = s.term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut line = Vec::new();
            let mut x = 0;
            while x < buf.area.width {
                let c = &buf[(x, y)];
                line.push((c.symbol().to_string(), c.fg, c.bg, c.modifier));
                x += (unicode_width::UnicodeWidthStr::width(c.symbol()) as u16).max(1);
            }
            line
        })
        .collect()
}

/// The first row where two drawn screens differ (text, then colours), described for a failure.
fn first_difference(a: &Sim, b: &Sim) -> Option<String> {
    let (ra, rb) = (rows(a), rows(b));
    if let Some(i) = ra.iter().zip(&rb).position(|(x, y)| x != y) {
        return Some(format!(
            "row {i} text:\n  sync:   {:?}\n  worker: {:?}",
            ra[i], rb[i]
        ));
    }
    let (ca, cb) = (cells(a), cells(b));
    ca.iter()
        .zip(&cb)
        .position(|(x, y)| x != y)
        .map(|i| format!("row {i} has the same text but different colours or modifiers"))
}

/// Opens `file` through the tree and waits until every job it started has landed.
#[track_caller]
fn open_settled(root: &std::path::Path, file: &str, worker: bool) -> Sim {
    let base = Sim::with_config_sized(root, cfg_en(), 110, 40);
    let mut s = if worker {
        base.with_media_and_preview_worker()
    } else {
        base.with_media()
    };
    s.select(file);
    s.enter();
    s.settle_inline_images(file, std::time::Duration::from_secs(120));
    s.draw();
    s
}

#[test]
fn every_sample_is_drawn_the_same_with_and_without_the_preview_worker() {
    let files = sample_files();
    assert!(
        files
            .iter()
            .filter(|f| f.to_string_lossy().contains("/code/"))
            .count()
            >= 15,
        "samples/code was not walked: {files:?}"
    );
    assert!(
        files.len() >= 55,
        "the samples directory was not read: {} files",
        files.len()
    );
    // A sample with a remote picture starts a download: its cache is a sandbox, never the real one.
    let cache = sandbox("pwp_cache");
    crate::test_support::set_test_cache_root(cache.to_path_buf());
    let mut failures = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let dir = sandbox("pwp");
        // A directory of samples is flattened into the sandbox: the file's own name decides its kind.
        std::fs::copy(f, dir.join(&name)).unwrap();
        let root = canon(&dir);
        let sync = open_settled(&root, &name, false);
        let worker = open_settled(&root, &name, true);
        if let Some(d) = first_difference(&sync, &worker) {
            failures.push(format!("{name}: drawn differently with the worker; {d}"));
        }
        // A table is drawn as a table (the title carries the row position), not as raw text.
        if name.ends_with(".csv") || name.ends_with(".tsv") {
            for (road, s) in [("sync", &sync), ("worker", &worker)] {
                if !s.screen().contains("r1/") {
                    failures.push(format!("{name}: the {road} road does not draw the table"));
                }
            }
        }
        // A file a standard plugin takes is never the failure screen on either road.
        for (road, s) in [("sync", &sync), ("worker", &worker)] {
            if s.screen().contains("can not preview") {
                failures.push(format!("{name}: the {road} road draws `can not preview`"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn with_the_worker_a_tree_enter_and_ctrl_n_open_markdown_then_a_table() {
    let dir = sandbox("pwp_keys");
    std::fs::write(dir.join("a.md"), "# Heading One\n\nbody text\n").unwrap();
    std::fs::write(dir.join("b.csv"), "col1,col2\n1,2\n3,4\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 110, 30).with_media_and_preview_worker();
    s.select("a.md");
    s.enter();
    s.settle_inline_images("a.md", std::time::Duration::from_secs(120));
    s.see("Heading One");
    s.dont_see("can not preview");
    s.ctrl('n');
    s.settle_inline_images("b.csv", std::time::Duration::from_secs(120));
    s.see("r1/");
    s.see("col1");
    s.dont_see("can not preview");
}

#[test]
fn with_the_worker_a_tree_enter_on_a_csv_draws_the_table() {
    let dir = sandbox("pwp_csv");
    std::fs::write(dir.join("b.csv"), "col1,col2\n1,2\n3,4\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 110, 30).with_media_and_preview_worker();
    s.select("b.csv");
    s.enter();
    s.settle_inline_images("b.csv", std::time::Duration::from_secs(120));
    s.see("r1/");
    s.see("col1");
}

// --- What an opener wants done once the kind is known (`Landing`), and a tab saved while its
// kind was being decided. Each runs the same operation on both roads and compares what is drawn.

fn road(root: &std::path::Path, cfg: Config, worker: bool) -> Sim {
    let base = Sim::with_config_sized(root, cfg, 100, 30);
    if worker {
        base.with_media_and_preview_worker()
    } else {
        base.with_media()
    }
}

#[track_caller]
fn assert_same_screen(sync: &Sim, worker: &Sim, what: &str) {
    if let Some(d) = first_difference(sync, worker) {
        panic!(
            "{what}: drawn differently with the worker; {d}\n--- sync\n{}\n--- worker\n{}",
            sync.screen(),
            worker.screen()
        );
    }
}

fn numbered_lines(n: usize) -> String {
    (1..=n).map(|i| format!("line number {i}\n")).collect()
}

#[test]
fn a_paste_jump_to_a_line_lands_on_it_with_and_without_the_worker() {
    let dir = sandbox("pwp_goto_text");
    std::fs::write(dir.join("long.txt"), numbered_lines(300)).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("long.txt"), Some(100));
        s.settle_inline_images("long.txt", std::time::Duration::from_secs(120));
        s.draw();
        s.see("line number 100");
        s.dont_see("can not preview");
        assert!(
            s.app.preview_top_line() > 0,
            "worker={worker}: the view did not move off the first line"
        );
        assert_eq!(s.app.preview_caret_line(), 99, "worker={worker}");
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "paste jump file:100");
}

#[test]
fn a_markdown_line_jump_lands_on_its_source_with_and_without_the_worker() {
    let dir = sandbox("pwp_goto_md");
    let mut body = String::from("# Title\n\n");
    for i in 0..60 {
        body.push_str(&format!("Paragraph {i} with some **bold** words.\n\n"));
    }
    std::fs::write(dir.join("doc.md"), body).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        s.settle_inline_images("doc.md", std::time::Duration::from_secs(120));
        s.draw();
        // The source is shown (the `**` markers are on screen), at the requested line.
        s.see("**bold**");
        s.see("Paragraph 19");
        s.dont_see("can not preview");
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "markdown file:40");
}

#[cfg(feature = "git")]
fn changed_repo(name: &str, file: &str, original: &str, changed: &str) -> std::path::PathBuf {
    let dir = sandbox(name);
    init_git_repo(&dir);
    std::fs::write(dir.join(file), original).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join(file), changed).unwrap();
    let root = canon(&dir);
    std::mem::forget(dir); // the sandbox guard must outlive both roads; the test dir is removed by the next run
    root
}

#[cfg(feature = "git")]
#[test]
fn a_follow_jump_lands_on_the_first_change_with_and_without_the_worker() {
    for (file, original, changed, marker) in [
        (
            "plain.txt",
            numbered_lines(300),
            numbered_lines(300).replace("line number 200\n", "CHANGED MARKER\n"),
            "CHANGED MARKER",
        ),
        (
            "doc.md",
            {
                let mut b = String::from("# Title\n\n");
                for i in 0..80 {
                    b.push_str(&format!(
                        "Filler paragraph number {i} to take up space.\n\n"
                    ));
                }
                b.push_str("ORIGINAL MARKER paragraph.\n");
                b
            },
            {
                let mut b = String::from("# Title\n\n");
                for i in 0..80 {
                    b.push_str(&format!(
                        "Filler paragraph number {i} to take up space.\n\n"
                    ));
                }
                b.push_str("CHANGED MARKER paragraph.\n");
                b
            },
            "CHANGED MARKER",
        ),
    ] {
        let root = changed_repo(&format!("pwp_follow_{file}"), file, &original, &changed);
        let mut screens = Vec::new();
        for worker in [false, true] {
            let mut cfg = cfg_en();
            cfg.ui.follow_view = "file".into();
            let mut s = road(&root, cfg, worker);
            s.key('F');
            let target = s.app.tab.root.join(file);
            assert!(s.app.follow_note_change(&target));
            s.app.follow_jump(&target);
            s.settle_inline_images(file, std::time::Duration::from_secs(120));
            s.draw();
            assert!(
                s.find_text(marker).is_some(),
                "{file} worker={worker}: the first change is not on screen:\n{}",
                s.screen()
            );
            screens.push(s);
        }
        assert_same_screen(&screens[0], &screens[1], &format!("follow {file}"));
    }
}

#[cfg(feature = "git")]
#[test]
fn the_diff_preview_representation_lands_on_the_first_change_with_and_without_the_worker() {
    let mut body = String::from("# Title\n\n");
    for i in 0..80 {
        body.push_str(&format!(
            "Filler paragraph number {i} to take up space.\n\n"
        ));
    }
    let original = format!("{body}ORIGINAL MARKER paragraph.\n");
    let changed = format!("{body}CHANGED MARKER paragraph.\n");
    let root = changed_repo("pwp_diff_preview", "doc.md", &original, &changed);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("doc.md");
        s.key('d');
        // The diff's target kind is answered by the worker too: wait for the answer.
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
        s.key('R'); // Rendered -> the diff's own Preview representation
        s.settle_inline_images("diff preview", std::time::Duration::from_secs(120));
        s.draw();
        assert!(
            s.app.preview_is_diff_representation(),
            "worker={worker}: R did not enter the Preview representation"
        );
        assert!(
            s.find_text("CHANGED MARKER").is_some(),
            "worker={worker}: the first change is not on screen:\n{}",
            s.screen()
        );
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "diff preview representation");
}

#[test]
fn restored_tabs_that_were_previews_are_drawn_when_switched_to_with_the_worker() {
    let dir = sandbox("pwp_restore");
    std::fs::write(dir.join("a.md"), "# Alpha Heading\n\nalpha body\n").unwrap();
    std::fs::write(dir.join("b.csv"), "col1,col2\n1,2\n").unwrap();
    let root = canon(&dir);
    let base = unique_tmp("konoma_pwp_restore_base");
    let _ = std::fs::remove_dir_all(&base);

    // Last time: a.md previewed in tab 1, b.csv previewed in tab 2, tab 1 on screen.
    let mut s = road(&root, cfg_en(), false);
    s.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &root,
        ));
    s.select("a.md");
    s.enter();
    s.key('t');
    s.select("b.csv");
    s.enter();
    s.key('[');
    s.app.save_session();
    drop(s);

    // This time, with the worker: every reply is still on its way when the tabs are restored.
    let mut s2 = road(&root, cfg_en(), true);
    s2.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &root,
        ));
    s2.app.restore_session();
    assert_eq!(s2.app.tab_count(), 2);
    s2.settle_inline_images("restored tab 1", std::time::Duration::from_secs(120));
    s2.see("Alpha Heading");
    s2.dont_see("can not preview");
    s2.key(']');
    s2.settle_inline_images("restored tab 2", std::time::Duration::from_secs(120));
    s2.see("col1");
    s2.dont_see("can not preview");
    s2.key('[');
    s2.settle_inline_images("tab 1 again", std::time::Duration::from_secs(120));
    s2.see("Alpha Heading");
    s2.dont_see("can not preview");
}

// --- A resolution still on its way belongs to the file preview that asked for it.

/// A file preview left (`q`) before the worker's reply must take its pending resolution with it,
/// even when the next thing opened is the *same file's* diff (so no path comparison can tell them
/// apart): a later tab round trip must not ask again about, and land on, that diff as a preview.
#[cfg(feature = "git")]
#[test]
fn a_left_previews_pending_landing_does_not_turn_the_next_diff_into_a_preview() {
    let root = changed_repo(
        "pwp_left_pending",
        "a.txt",
        &numbered_lines(300),
        &format!("changed first\n{}", numbered_lines(300)),
    );
    let mut s = road(&root, cfg_en(), true);
    // The file is opened on line 100: its kind goes to the worker and the reply is not taken yet.
    s.app.paste_jump_to(&root.join("a.txt"), Some(100));
    s.key('q');
    s.select("a.txt");
    s.key('d');
    // A tab round trip: the tab is saved and made active again.
    s.key('t');
    s.key('[');
    s.settle_inline_images("round trip", std::time::Duration::from_secs(60));
    // Whatever replies were on their way, the diff is still the diff.
    while let Ok(res) = s.media_rx.as_ref().unwrap().try_recv() {
        s.app.apply_media(res);
    }
    s.draw();
    assert!(
        s.app.is_git_diff_preview(),
        "the diff became an ordinary preview:\n{}",
        s.screen()
    );
}

// --- A landing waits for the document's text.

/// The plugin has not delivered a Markdown file's text when the preview opens (the first frame's
/// wait is over): the line the opener asked for is not lost, it lands with the text. The plugin's
/// `open` is held behind a gate, so the state is made, not raced.
#[test]
fn a_line_jump_into_markdown_whose_text_comes_late_lands_when_it_comes() {
    use crate::app::HoldOpens;
    use crate::plugins::test_plugins::Latch;
    let dir = sandbox("pwp_goto_md_late");
    let mut body = String::from("# Title\n\n");
    for i in 0..60 {
        body.push_str(&format!("Paragraph {i} with some **bold** words.\n\n"));
    }
    std::fs::write(dir.join("doc.md"), body).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let gate = std::sync::Arc::new(Latch::default());
        let _hold = HoldOpens::until(&gate);
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        // With the worker the kind is decided by a reply: take it (it opens the plugin).
        while worker && !s.app.is_document_loading() {
            let res = s
                .media_rx
                .as_ref()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("the worker answers");
            s.app.apply_media(res);
        }
        assert!(
            s.app.document_text_missing(),
            "worker={worker}: the text must still be on its way"
        );
        gate.open();
        s.settle_inline_images("late markdown", std::time::Duration::from_secs(120));
        s.draw();
        s.see("**bold**");
        s.see("Paragraph 19");
        s.dont_see("can not preview");
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "markdown file:40, late text");
}

// --- A screen that covered a preview gives it back where it was read.

/// Opens `file` on `road`, reads it down to `line`, covers it with a project screen and leaves
/// that screen again: the position of the reading is what the preview is back at.
#[track_caller]
fn read_cover_and_return(root: &std::path::Path, file: &str, line: usize, worker: bool) -> Sim {
    let mut s = road(root, cfg_en(), worker);
    s.select(file);
    s.enter();
    s.settle_inline_images("opened", std::time::Duration::from_secs(60));
    s.app.paste_jump_to(&root.join(file), Some(line));
    s.settle_inline_images("jumped", std::time::Duration::from_secs(60));
    s.draw();
    s.app.cover_with_project_screen_for_test();
    s.app.leave_project_screens_for_test();
    s.settle_inline_images("returned", std::time::Duration::from_secs(60));
    s.draw();
    s
}

#[test]
fn a_windowed_text_is_read_where_it_was_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_text");
    std::fs::write(dir.join("long.txt"), numbered_lines(300)).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let s = read_cover_and_return(&root, "long.txt", 150, worker);
        s.see("line number 150");
        assert!(
            s.app.preview_top_line() > 0,
            "worker={worker}: back at the first line"
        );
        assert_eq!(s.app.preview_caret_line(), 149, "worker={worker}");
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "text after a project screen");
}

#[test]
fn a_markdown_view_is_read_where_it_was_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_md");
    let mut body = String::from("# Title\n\n");
    for i in 0..60 {
        body.push_str(&format!("Paragraph {i} with some **bold** words.\n\n"));
    }
    std::fs::write(dir.join("doc.md"), body).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("doc.md");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        let at_top = s.app.reading_position_for_test();
        // Rendered view, scrolled down.
        for _ in 0..30 {
            s.key('j');
        }
        s.draw();
        let before = s.app.reading_position_for_test();
        assert_ne!(before, at_top, "worker={worker}: the view did not move");
        s.app.cover_with_project_screen_for_test();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(
            s.app.reading_position_for_test(),
            before,
            "worker={worker}: not back where it was read"
        );
    }
}

// --- The pending landing and the reading position belong to one preview.

/// With the worker, takes the preview worker's replies until the plugin is asked for the
/// document (the kind is decided and the text is on its way).
#[track_caller]
fn take_replies_until_document_loading(s: &mut Sim) {
    while !s.app.is_document_loading() {
        let res = s
            .media_rx
            .as_ref()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the worker answers");
        s.app.apply_media(res);
    }
}

fn long_markdown() -> String {
    let mut body = String::from("# Title\n\n");
    for i in 0..60 {
        body.push_str(&format!("Paragraph {i} with some **bold** words.\n\n"));
    }
    body
}

/// A tab left while its Markdown text was on its way, and come back to after the text is ready
/// (it lands inside the first wait of the reload): the line the opener asked for lands. The
/// landing belongs to the tab that holds it, not to the tab that was showing when the text came.
#[test]
fn a_line_waiting_for_the_text_lands_after_a_tab_round_trip_with_and_without_the_worker() {
    use crate::app::{HoldOpens, LongFirstFrame};
    use crate::plugins::test_plugins::Latch;
    let dir = sandbox("pwp_goto_md_tabs");
    std::fs::write(dir.join("doc.md"), long_markdown()).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let gate = std::sync::Arc::new(Latch::default());
        let _hold = HoldOpens::until(&gate);
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        if worker {
            take_replies_until_document_loading(&mut s);
        }
        assert!(
            s.app.document_text_missing(),
            "worker={worker}: the text must still be on its way"
        );
        s.key('t'); // another tab: this one is saved with its landing
        gate.open();
        let _long = LongFirstFrame::of(std::time::Duration::from_secs(60));
        s.key('['); // back: the reload's text lands within the first wait
        s.settle_inline_images("round trip", std::time::Duration::from_secs(120));
        s.draw();
        s.see("**bold**");
        s.see("Paragraph 19");
        s.dont_see("can not preview");
        screens.push(s);
    }
    assert_same_screen(
        &screens[0],
        &screens[1],
        "markdown file:40 over a tab round trip",
    );
}

/// A document that could not be read ends the wait for its text: the line the opener asked for is
/// dropped with it, and a later successful reload shows the document from the top.
#[test]
fn a_line_asked_of_a_document_that_failed_is_not_landed_by_a_later_reload() {
    use std::os::unix::fs::PermissionsExt;
    let dir = sandbox("pwp_goto_md_failed");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, long_markdown()).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        s.settle_inline_images("failed", std::time::Duration::from_secs(120));
        s.draw();
        assert!(
            s.app.document_error().is_some(),
            "worker={worker}: the unreadable document must have failed:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.app.reload_preview();
        s.settle_inline_images("reloaded", std::time::Duration::from_secs(120));
        s.draw();
        s.see("Paragraph 0");
        s.dont_see("**bold**");
        s.dont_see("Paragraph 19");
        assert_eq!(s.app.preview_top_line(), 0, "worker={worker}");
    }
}

/// The same when the failure comes after the first wait: the line waits for the text (registered
/// while the document is still on its way), the read then fails, and the failure drops it.
#[test]
fn a_line_waiting_for_a_document_that_then_fails_is_not_landed_by_a_later_reload() {
    use crate::app::HoldOpens;
    use crate::plugins::test_plugins::Latch;
    use std::os::unix::fs::PermissionsExt;
    let dir = sandbox("pwp_goto_md_late_failed");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, long_markdown()).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        let gate = std::sync::Arc::new(Latch::default());
        let _hold = HoldOpens::until(&gate);
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        if worker {
            take_replies_until_document_loading(&mut s);
        }
        assert!(
            s.app.document_text_missing() && s.app.document_error().is_none(),
            "worker={worker}: the text must still be on its way"
        );
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
        gate.open();
        s.settle_inline_images("failed", std::time::Duration::from_secs(120));
        s.draw();
        assert!(
            s.app.document_error().is_some(),
            "worker={worker}: the document must have failed:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.app.reload_preview();
        s.settle_inline_images("reloaded", std::time::Duration::from_secs(120));
        s.draw();
        s.see("Paragraph 0");
        s.dont_see("**bold**");
        s.dont_see("Paragraph 19");
        assert_eq!(s.app.preview_top_line(), 0, "worker={worker}");
    }
}

/// A windowed text is read at a line, not at a byte: the file changed while a project screen
/// covered it (the lines before the reading position changed length), and the preview comes
/// back at the same line of the *new* text, its numbers and caret in agreement.
#[test]
fn a_windowed_text_changed_under_a_project_screen_is_read_at_the_same_line() {
    let dir = sandbox("pwp_return_changed");
    std::fs::write(dir.join("long.txt"), numbered_lines(400)).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        std::fs::write(dir.join("long.txt"), numbered_lines(400)).unwrap();
        let mut s = road(&root, cfg_en(), worker);
        s.select("long.txt");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        s.app.paste_jump_to(&root.join("long.txt"), Some(200));
        s.settle_inline_images("jumped", std::time::Duration::from_secs(60));
        s.draw();
        let top = s.app.preview_top_line();
        assert!(top > 100, "worker={worker}: read down to line {top}");
        s.app.cover_with_project_screen_for_test();
        // Every line is longer than before, so no byte of the old file is a line head here.
        let changed: String = (1..=400)
            .map(|i| format!("new text of line {i} with a longer tail than before\n"))
            .collect();
        std::fs::write(dir.join("long.txt"), changed).unwrap();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(s.app.preview_top_line(), top, "worker={worker}");
        assert_eq!(s.app.preview_caret_line(), 199, "worker={worker}");
        s.dont_see("line number");
        // The first shown row is the head of line `top + 1` of the new text, numbered as it is.
        let first = format!("new text of line {} with", top + 1);
        let row = rows(&s)
            .into_iter()
            .find(|r| r.contains("new text of line"))
            .expect("a text row");
        assert!(
            row.contains(&first),
            "worker={worker}: first row is {row:?}, want line {}",
            top + 1
        );
        s.see("new text of line 200 with");
        screens.push(s);
    }
    assert_same_screen(
        &screens[0],
        &screens[1],
        "changed text after a project screen",
    );
}

/// The representation a tab chose is part of where it was read: a Markdown file read as raw
/// source (`R`) comes back from a project screen as raw source, at the same line.
#[test]
fn a_raw_markdown_view_is_read_where_it_was_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_md_raw");
    std::fs::write(dir.join("doc.md"), long_markdown()).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("doc.md");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        s.key('R');
        s.settle_inline_images("raw", std::time::Duration::from_secs(60));
        for _ in 0..60 {
            s.key('j');
        }
        s.draw();
        assert!(
            s.app.is_md_raw(),
            "worker={worker}: R did not show the source"
        );
        let before = rows(&s);
        let top = s.app.preview_top_line();
        let caret = s.app.preview_caret_line();
        assert!(top > 10, "worker={worker}: the view did not move ({top})");
        s.app.cover_with_project_screen_for_test();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert!(
            s.app.is_md_raw(),
            "worker={worker}: back in the rendered view:\n{}",
            s.screen()
        );
        assert_eq!(s.app.preview_top_line(), top, "worker={worker}");
        assert_eq!(s.app.preview_caret_line(), caret, "worker={worker}");
        assert_eq!(rows(&s), before, "worker={worker}: drawn differently");
        screens.push(s);
    }
    assert_same_screen(
        &screens[0],
        &screens[1],
        "raw markdown after a project screen",
    );
}

/// A table is read at a cell: the cell cursor and the scroll of the view come back after a
/// project screen, as they do after a tab switch.
#[test]
fn a_table_is_read_at_the_same_cell_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_csv");
    let mut csv = String::new();
    for r in 0..80 {
        let row: Vec<String> = (0..30).map(|c| format!("r{r}c{c}")).collect();
        csv.push_str(&row.join(","));
        csv.push('\n');
    }
    std::fs::write(dir.join("grid.csv"), csv).unwrap();
    let root = canon(&dir);
    let mut screens = Vec::new();
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("grid.csv");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        s.app.table_cursor_move(50, 20);
        s.draw();
        let cursor = s.app.table_cursor();
        let scroll = s.app.table_scroll();
        assert_eq!(cursor, (50, 20), "worker={worker}");
        assert!(scroll.0 > 0 && scroll.1 > 0, "worker={worker}: {scroll:?}");
        let before = rows(&s);
        s.app.cover_with_project_screen_for_test();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(s.app.table_cursor(), cursor, "worker={worker}: cell");
        assert_eq!(s.app.table_scroll(), scroll, "worker={worker}: scroll");
        assert_eq!(rows(&s), before, "worker={worker}: drawn differently");
        screens.push(s);
    }
    assert_same_screen(&screens[0], &screens[1], "table after a project screen");
}

/// A PDF is read at a page: page 3 of a multi-page document comes back as page 3.
#[test]
fn a_pdf_is_read_on_the_same_page_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_pdf");
    let pages: Vec<(u32, u32, &[u8])> = vec![
        (200, 300, b"1 0 0 rg 0 0 200 300 re f"),
        (200, 300, b"0 0 1 rg 0 0 200 300 re f"),
        (200, 300, b"0 1 0 rg 0 0 200 300 re f"),
        (200, 300, b"1 1 0 rg 0 0 200 300 re f"),
    ];
    std::fs::write(dir.join("doc.pdf"), konoma_test_support::build_pdf(&pages)).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("doc.pdf");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        s.app.page_turn(1);
        s.settle_inline_images("page 2", std::time::Duration::from_secs(60));
        s.app.page_turn(1);
        s.settle_inline_images("page 3", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(s.app.page_indicator(), Some((3, 4)), "worker={worker}");
        s.app.cover_with_project_screen_for_test();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(
            s.app.page_indicator(),
            Some((3, 4)),
            "worker={worker}: not on page 3:\n{}",
            s.screen()
        );
    }
}

/// An image is read at a zoom and a centre: they come back after a project screen.
#[test]
fn an_image_is_read_at_the_same_zoom_after_a_project_screen_with_and_without_the_worker() {
    let dir = sandbox("pwp_return_png");
    std::fs::copy(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/sample.png"),
        dir.join("pic.png"),
    )
    .unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        let mut s = road(&root, cfg_en(), worker);
        s.select("pic.png");
        s.enter();
        s.settle_inline_images("opened", std::time::Duration::from_secs(60));
        s.app.image_zoom_by(2.0);
        s.app.tab.image_view.center = (0.3, 0.7);
        let view = s.app.tab.image_view;
        assert!(view.zoom > 1.0, "worker={worker}: {view:?}");
        s.app.cover_with_project_screen_for_test();
        s.app.leave_project_screens_for_test();
        s.settle_inline_images("returned", std::time::Duration::from_secs(60));
        s.draw();
        assert_eq!(s.app.tab.image_view, view, "worker={worker}");
    }
}

/// The landing waiting in the tab that was showing is not run on the tab that is switched to,
/// even when both show the same file: tab 1 reads `doc.md`, tab 2 has `doc.md:40` waiting for its
/// text; the text of tab 1's reload lands inside the first wait. Tab 1 stays the rendered view,
/// and tab 2 still lands its own line when it is back.
#[test]
fn a_waiting_landing_is_not_run_on_another_tab_of_the_same_file() {
    use crate::app::{HoldOpens, LongFirstFrame};
    use crate::plugins::test_plugins::Latch;
    let dir = sandbox("pwp_goto_md_two_tabs");
    std::fs::write(dir.join("doc.md"), long_markdown()).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        let gate = std::sync::Arc::new(Latch::default());
        let _hold = HoldOpens::until(&gate);
        let mut s = road(&root, cfg_en(), worker);
        s.select("doc.md");
        s.enter();
        if worker {
            take_replies_until_document_loading(&mut s);
        }
        s.key('t');
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        if worker {
            take_replies_until_document_loading(&mut s);
        }
        assert!(
            s.app.document_text_missing(),
            "worker={worker}: the text must still be on its way"
        );
        gate.open();
        let _long = LongFirstFrame::of(std::time::Duration::from_secs(60));
        s.key('['); // tab 1: its text lands inside the first wait of the reload
        s.settle_inline_images("tab 1", std::time::Duration::from_secs(120));
        s.draw();
        s.see("Paragraph 0 with some bold words");
        s.dont_see("**bold**");
        s.key(']'); // tab 2: its own landing runs
        s.settle_inline_images("tab 2", std::time::Duration::from_secs(120));
        s.draw();
        s.see("**bold**");
        s.see("Paragraph 19");
    }
}

// --- One restore for a covered preview (a project screen) and a left tab (a tab switch).

/// Opens `file` and reads it down to `line` (1-based), settled and drawn.
#[track_caller]
fn open_at_line(root: &std::path::Path, file: &str, line: usize, worker: bool) -> Sim {
    let mut s = road(root, cfg_en(), worker);
    s.select(file);
    s.enter();
    s.settle_inline_images("opened", std::time::Duration::from_secs(60));
    s.app.paste_jump_to(&root.join(file), Some(line));
    s.settle_inline_images("jumped", std::time::Duration::from_secs(60));
    s.draw();
    s
}

/// A file that got shorter while covered: the caret stops on the last line that exists (a file of
/// 10 lines ending with a newline has lines 0..=9), and `Y` names line 10, never a line past the
/// end. The tab switch puts a tab back through the same function, so it gives the same.
#[test]
fn a_file_shortened_while_covered_puts_the_caret_on_its_last_line() {
    let dir = sandbox("pwp_return_shortened");
    let root = canon(&dir);
    for worker in [false, true] {
        for via_tab in [false, true] {
            std::fs::write(dir.join("long.txt"), numbered_lines(400)).unwrap();
            let mut s = open_at_line(&root, "long.txt", 200, worker);
            assert_eq!(s.app.preview_caret_line(), 199);
            let what = format!("worker={worker} via_tab={via_tab}");
            if via_tab {
                s.key('t');
                std::fs::write(dir.join("long.txt"), numbered_lines(10)).unwrap();
                s.key('[');
            } else {
                s.app.cover_with_project_screen_for_test();
                std::fs::write(dir.join("long.txt"), numbered_lines(10)).unwrap();
                s.app.leave_project_screens_for_test();
            }
            s.settle_inline_images("returned", std::time::Duration::from_secs(60));
            s.draw();
            s.see("line number 10");
            assert_eq!(s.app.preview_caret_line(), 9, "{what}");
            assert_eq!(
                s.app.selection_ref_string().as_deref(),
                Some("@long.txt#L10"),
                "{what}"
            );
        }
    }
}

/// An unchanged windowed file (same length and mtime) keeps its byte offset: it is put back
/// without finding the line again (no scan of the file from its start). The seam is the
/// `LINE_SEEKS` counter in `seek_windowed_line`, the only place that finds a line in a file.
#[test]
fn an_unchanged_windowed_file_is_put_back_without_a_line_seek() {
    let dir = sandbox("pwp_return_unchanged");
    std::fs::write(dir.join("long.txt"), numbered_lines(400)).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        for via_tab in [false, true] {
            let mut s = open_at_line(&root, "long.txt", 200, worker);
            let top = s.app.preview_top_line();
            let body = |s: &Sim| {
                let r = rows(s);
                r[2..r.len() - 2].to_vec() // not the tab bar (two tabs after `t`) nor the footer
            };
            let before = body(&s);
            let what = format!("worker={worker} via_tab={via_tab}");
            crate::app::LINE_SEEKS.with(|c| c.set(0));
            if via_tab {
                s.key('t');
                s.key('[');
            } else {
                s.app.cover_with_project_screen_for_test();
                s.app.leave_project_screens_for_test();
            }
            s.settle_inline_images("returned", std::time::Duration::from_secs(60));
            s.draw();
            assert_eq!(s.app.preview_top_line(), top, "{what}");
            assert_eq!(s.app.preview_caret_line(), 199, "{what}");
            assert_eq!(body(&s), before, "{what}: drawn differently");
            assert_eq!(
                crate::app::LINE_SEEKS.with(|c| c.get()),
                0,
                "{what}: the unchanged file was scanned for its line again"
            );
        }
    }
}

/// A document that fails inside the reload of the tab that is put back ends the wait of THAT
/// tab: the line it had waiting is dropped with the failure, and a later successful reload shows
/// the document from the top (the restore must not bring the landing back).
#[test]
fn a_document_failing_in_the_tab_return_drops_that_tabs_waiting_line() {
    use crate::app::HoldOpens;
    use crate::plugins::test_plugins::Latch;
    use std::os::unix::fs::PermissionsExt;
    let dir = sandbox("pwp_goto_md_tab_failed");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, long_markdown()).unwrap();
    let root = canon(&dir);
    for worker in [false, true] {
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        let gate = std::sync::Arc::new(Latch::default());
        let _hold = HoldOpens::until(&gate);
        let mut s = road(&root, cfg_en(), worker);
        s.app.paste_jump_to(&root.join("doc.md"), Some(40));
        if worker {
            take_replies_until_document_loading(&mut s);
        }
        assert!(
            s.app.document_text_missing() && s.app.document_error().is_none(),
            "worker={worker}: the text must still be on its way"
        );
        s.key('t'); // this tab is saved with its line waiting
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
        gate.open();
        s.key('['); // back: the reload of the restored tab fails
        s.settle_inline_images("failed", std::time::Duration::from_secs(120));
        s.draw();
        assert!(
            s.app.document_error().is_some(),
            "worker={worker}: the document must have failed:\n{}",
            s.screen()
        );
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.app.reload_preview();
        s.settle_inline_images("reloaded", std::time::Duration::from_secs(120));
        s.draw();
        s.see("Paragraph 0");
        s.dont_see("**bold**");
        s.dont_see("Paragraph 19");
        assert_eq!(s.app.preview_top_line(), 0, "worker={worker}");
    }
}

// --- The corpus: every kind of preview state comes back the same from a project screen and from a tab.

/// Everything a reader sees (the drawn rows but the tab bar and the footer) and everything the keys act on.
fn reading_state(s: &Sim) -> String {
    let r = rows(s);
    let a = &s.app;
    format!(
        "{:#?}\nkind={:?} pos={:?} search={:?}/{:?} fence=({}) page={:?} \
         table={:?}/{:?} raw={} image={:?} ref={:?} picture={:?}",
        &r[1..r.len() - 1], // not the tab bar, nor the footer (a notice of the jump that is gone)
        a.tab.preview_kind,
        a.reading_position_for_test(),
        a.search_status(),
        a.preview_search_query(),
        a.fence_state_for_test(),
        a.page_indicator(),
        a.table_cursor(),
        a.table_scroll(),
        a.is_md_raw(),
        a.tab.image_view,
        a.selection_ref_string(),
        a.picture_centre_rgb_for_test(),
    )
}

fn settled(s: &mut Sim) {
    s.settle_inline_images("settled", std::time::Duration::from_secs(120));
    s.draw();
}

/// One preview state of the corpus: the files, how a reader gets into the state, and what a
/// reader does next (the keys act on the state that was put back).
struct Case {
    name: &'static str,
    files: fn(&std::path::Path),
    cfg: fn() -> Config,
    open: fn(&mut Sim, &std::path::Path),
    probe: fn(&mut Sim),
    /// A check that the state really is the one the case claims (not a state that came out empty).
    sanity: fn(&Sim),
}

fn plain_cfg() -> Config {
    cfg_en()
}

fn fence_markdown() -> String {
    let mut src = String::from("# Diagram\n\n```mermaid\ngraph LR\n  A --> B\n  B --> C\n```\n\n");
    for i in 0..60 {
        src.push_str(&format!("filler paragraph {i}\n\n"));
    }
    src
}

fn open_file(s: &mut Sim, file: &str) {
    s.select(file);
    s.enter();
    settled(s);
}

fn open_fence(s: &mut Sim) {
    open_file(s, "fence.md");
    s.app.md_focus_move(1);
    s.draw();
}

fn pdf_files(dir: &std::path::Path) {
    let pages: Vec<(u32, u32, &[u8])> = vec![
        (200, 300, b"1 0 0 rg 0 0 200 300 re f"),
        (200, 300, b"0 0 1 rg 0 0 200 300 re f"),
        (200, 300, b"0 1 0 rg 0 0 200 300 re f"),
        (200, 300, b"1 1 0 rg 0 0 200 300 re f"),
    ];
    std::fs::write(dir.join("doc.pdf"), konoma_test_support::build_pdf(&pages)).unwrap();
}

fn corpus() -> Vec<Case> {
    vec![
        Case {
            name: "zoomed and panned mermaid fence",
            files: |d| std::fs::write(d.join("fence.md"), fence_markdown()).unwrap(),
            cfg: plain_cfg,
            open: |s, _| {
                open_fence(s);
                s.app.image_zoom_by(4.0);
                s.key('l');
                s.key('j');
                s.draw();
            },
            probe: |s| {
                s.key('l');
                s.key('l');
                s.draw();
            },
            sanity: |s| {
                assert!(s.app.fence_zoom_level() > 3.9);
                assert!(!s.app.fence_state_for_test().contains("center=(0.5, 0.5)"));
            },
        },
        Case {
            name: "full-screen mermaid fence",
            files: |d| std::fs::write(d.join("fence.md"), fence_markdown()).unwrap(),
            cfg: plain_cfg,
            open: |s, _| {
                open_fence(s);
                s.enter();
                settled(s);
            },
            probe: |s| {
                s.key('q'); // back to the document, where the reader was
                settled(s);
            },
            sanity: |s| {
                assert!(s.app.is_full_screen_fence_for_test());
                assert!(!s.app.fence_state_for_test().contains("return=None"));
            },
        },
        Case {
            name: "confirmed search",
            files: |d| std::fs::write(d.join("long.txt"), numbered_lines(400)).unwrap(),
            cfg: plain_cfg,
            open: |s, _| {
                open_file(s, "long.txt");
                s.key('/');
                s.keys("number 3");
                s.enter();
                s.key('n');
                s.key('n');
                s.draw();
            },
            probe: |s| {
                s.key('n');
                s.key('N');
                s.key('N');
                s.draw();
            },
            sanity: |s| {
                let (i, n) = s.app.search_status().expect("a search with hits");
                assert!(n > 5 && i > 1, "{i}/{n}");
            },
        },
        Case {
            name: "command text read half way",
            files: |d| std::fs::write(d.join("app.log"), numbered_lines(300)).unwrap(),
            cfg: || {
                let mut cfg = cfg_en();
                cfg.preview.rules.insert(
                    0,
                    crate::config::Rule {
                        glob: Some("*.log".into()),
                        command: Some("cat {path}".into()),
                        ..Default::default()
                    },
                );
                cfg
            },
            open: |s, root| {
                open_file(s, "app.log");
                s.app.paste_jump_to(&root.join("app.log"), Some(150));
                settled(s);
            },
            probe: |s| {
                s.key('j');
                s.key('j');
                s.draw();
            },
            sanity: |s| {
                assert!(s.app.preview_top_line() > 100);
                assert_eq!(s.app.preview_caret_line(), 149);
            },
        },
        Case {
            name: "windowed large text",
            files: |d| std::fs::write(d.join("long.txt"), numbered_lines(400)).unwrap(),
            cfg: plain_cfg,
            open: |s, root| {
                open_file(s, "long.txt");
                s.app.paste_jump_to(&root.join("long.txt"), Some(250));
                settled(s);
                s.key('j');
                s.key('l');
            },
            probe: |s| {
                s.key('j');
                s.key('j');
                s.draw();
            },
            sanity: |s| assert_eq!(s.app.preview_caret_line(), 250),
        },
        Case {
            name: "raw markdown",
            files: |d| std::fs::write(d.join("doc.md"), long_markdown()).unwrap(),
            cfg: plain_cfg,
            open: |s, _| {
                open_file(s, "doc.md");
                s.key('R');
                settled(s);
                for _ in 0..40 {
                    s.key('j');
                }
                s.draw();
            },
            probe: |s| {
                s.key('j');
                s.key('R'); // back to the rendered view
                settled(s);
            },
            sanity: |s| assert!(s.app.is_md_raw()),
        },
        Case {
            name: "csv cell",
            files: |d| {
                let mut csv = String::new();
                for r in 0..80 {
                    let row: Vec<String> = (0..30).map(|c| format!("r{r}c{c}")).collect();
                    csv.push_str(&row.join(","));
                    csv.push('\n');
                }
                std::fs::write(d.join("grid.csv"), csv).unwrap();
            },
            cfg: plain_cfg,
            open: |s, _| {
                open_file(s, "grid.csv");
                s.app.table_cursor_move(50, 20);
                s.draw();
            },
            probe: |s| {
                s.key('j');
                s.key('l');
                s.draw();
            },
            sanity: |s| assert_eq!(s.app.table_cursor(), (50, 20)),
        },
        Case {
            name: "pdf on page 3",
            files: pdf_files,
            cfg: plain_cfg,
            open: |s, _| {
                open_file(s, "doc.pdf");
                s.app.page_turn(1);
                settled(s);
                s.app.page_turn(1);
                settled(s);
            },
            probe: |s| {
                s.app.page_turn(1);
                settled(s);
            },
            sanity: |s| {
                assert_eq!(s.app.page_indicator(), Some((3, 4)));
                // The picture drawn is page 3's (green), not the page the indicator might lie about.
                let (r, g, b) = s
                    .app
                    .picture_centre_rgb_for_test()
                    .unwrap_or_else(|| panic!("no picture:\n{}", s.screen()));
                assert!(g > 200 && r < 60 && b < 60, "{:?}", (r, g, b));
            },
        },
        Case {
            name: "zoomed image",
            files: |d| {
                std::fs::copy(
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/sample.png"),
                    d.join("pic.png"),
                )
                .unwrap();
            },
            cfg: plain_cfg,
            open: |s, _| {
                open_file(s, "pic.png");
                s.app.image_zoom_by(2.0);
                s.app.tab.image_view.center = (0.3, 0.7);
                s.draw();
            },
            probe: |s| {
                s.app.image_pan(1.0, 0.0);
                s.draw();
            },
            sanity: |s| assert!(s.app.tab.image_view.zoom > 1.0),
        },
    ]
}

/// A case on one road: the reading state before, after a project screen, and after a tab round
/// trip, and the state after the next keys in each.
fn run_case(case: &Case, root: &std::path::Path, worker: bool) {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Trip {
        None,
        ProjectScreen,
        Tab,
    }
    let mut states: Vec<(Trip, String, String)> = Vec::new();
    for trip in [Trip::None, Trip::ProjectScreen, Trip::Tab] {
        let mut s = road(root, (case.cfg)(), worker);
        (case.open)(&mut s, root);
        settled(&mut s);
        (case.sanity)(&s);
        match trip {
            Trip::None => {}
            Trip::ProjectScreen => {
                s.app.cover_with_project_screen_for_test();
                s.app.leave_project_screens_for_test();
            }
            Trip::Tab => {
                s.key('t');
                s.key('[');
            }
        }
        settled(&mut s);
        let back = reading_state(&s);
        (case.probe)(&mut s);
        settled(&mut s);
        states.push((trip, back, reading_state(&s)));
    }
    let (_, before, after_keys) = states[0].clone();
    for (trip, back, keys) in &states[1..] {
        assert_eq!(
            back, &before,
            "{} (worker={worker}): after {trip:?} the preview is not as it was",
            case.name
        );
        assert_eq!(
            keys, &after_keys,
            "{} (worker={worker}): after {trip:?} the keys act differently",
            case.name
        );
    }
}

#[test]
fn every_kind_of_preview_state_comes_back_the_same_from_a_project_screen_and_a_tab() {
    let cases = corpus();
    assert!(cases.len() >= 9, "the corpus shrank");
    let mut failures = Vec::new();
    for case in &cases {
        let dir = sandbox("pwp_corpus");
        (case.files)(&dir);
        let root = canon(&dir);
        for worker in [false, true] {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_case(case, &root, worker)
            }));
            if let Err(e) = r {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                failures.push(msg.chars().take(1500).collect::<String>());
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n=====\n"));
}

/// The diff's target kind is answered by the worker too: wait for the answer.
#[cfg(feature = "git")]
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

/// A diff opened from the tree is read again from git on the way back (its own path), at the
/// scroll it had, on both roads.
#[cfg(feature = "git")]
#[test]
fn a_diff_opened_from_the_tree_comes_back_from_a_project_screen_and_a_tab() {
    let mut body = String::new();
    for i in 0..120 {
        body.push_str(&format!("filler line number {i}\n"));
    }
    let original = format!("{body}ORIGINAL MARKER\n");
    let changed = format!("{body}CHANGED MARKER\n");
    let root = changed_repo("pwp_corpus_diff", "doc.txt", &original, &changed);
    for worker in [false, true] {
        let mut states = Vec::new();
        for trip in 0..3 {
            let mut s = road(&root, cfg_en(), worker);
            s.select("doc.txt");
            s.key('d');
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            assert!(
                s.app.is_git_diff_preview(),
                "worker={worker}:\n{}",
                s.screen()
            );
            s.key('j');
            s.key('j');
            s.draw();
            match trip {
                1 => {
                    s.app.cover_with_project_screen_for_test();
                    s.app.leave_project_screens_for_test();
                }
                2 => {
                    s.key('t');
                    s.key('[');
                }
                _ => {}
            }
            wait_diff_target(&mut s, worker);
            settled(&mut s);
            assert!(
                s.app.is_git_diff_preview(),
                "worker={worker} trip={trip}:\n{}",
                s.screen()
            );
            let back = reading_state(&s);
            s.key('j');
            settled(&mut s);
            states.push((back, reading_state(&s)));
        }
        assert_eq!(
            states[1], states[0],
            "worker={worker}: after a project screen"
        );
        assert_eq!(
            states[2], states[0],
            "worker={worker}: after a tab round trip"
        );
    }
}
