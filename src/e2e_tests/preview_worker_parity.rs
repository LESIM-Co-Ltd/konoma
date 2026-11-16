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
