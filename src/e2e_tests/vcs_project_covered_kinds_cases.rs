// Every async result kind and timer of the run loop (`run` in src/lib.rs), delivered while the tab
// it was asked for is away: under a project screen (`p` ... `q`), and on another tab (a tab switch
// away and back). Covering is leaving the tab, so on both roads the result is applied to App-level
// state or dropped, the screen in front does not change, and the screen after the return is the
// same. The list of kinds is pinned against the run loop's source by
// `every_async_result_kind_and_timer_of_the_run_loop_has_a_covered_test` (src/lib.rs).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KindRoad {
    Cover,
    TabSwitch,
}

fn kind_away(sim: &mut Sim, road: KindRoad, kind: &str) {
    match road {
        KindRoad::Cover => {
            press(sim, KeyCode::Char('p'));
            drain(sim);
            assert!(sim.app.project_active(), "{kind}: p covers the tab\n{}", sim.screen());
        }
        KindRoad::TabSwitch => {
            sim.app.tab_cycle(1);
            drain(sim);
            assert_eq!(sim.app.active_tab_index(), 1, "{kind}: on the other tab");
        }
    }
}

fn kind_back(sim: &mut Sim, road: KindRoad, kind: &str) {
    match road {
        KindRoad::Cover => {
            press(sim, KeyCode::Char('q'));
            assert!(!sim.app.project_active(), "{kind}: q left the project screen");
        }
        KindRoad::TabSwitch => {
            sim.app.tab_cycle(-1);
            drain(sim);
            assert_eq!(sim.app.active_tab_index(), 0, "{kind}: back on the first tab");
        }
    }
}

/// The screen in front, without what is App-level by the spec: the flash line's message (the
/// office launch and the file operation tell their outcome there whatever is in front) and the
/// header's busy label (a job in flight, whichever tab asked it). The cells a wide character or a
/// cell of a forced width covers, the cells the frame skips, and the rest of a row after a cell
/// that writes escapes (a kitty picture's placeholder row, which moves the cursor itself) are left
/// out: the test backend keeps the symbol an earlier frame left there, which a terminal does not
/// show.
fn kind_front(sim: &mut Sim) -> String {
    let flash = sim.app.flash.take();
    sim.draw();
    sim.app.flash = flash;
    let buf = sim.term.backend().buffer();
    let w = buf.area.width as usize;
    let mut lines = Vec::new();
    let mut line = String::new();
    use ratatui::buffer::CellDiffOption;
    let mut covered = 0usize;
    for (i, cell) in buf.content().iter().enumerate() {
        if covered > 0 {
            covered -= 1;
        } else if cell.diff_option != CellDiffOption::Skip {
            line.push_str(cell.symbol());
            let width = match cell.diff_option {
                _ if cell.symbol().contains('\u{1b}') => w,
                CellDiffOption::ForcedWidth(w) => usize::from(w.get()),
                _ => unicode_width::UnicodeWidthStr::width(cell.symbol()),
            };
            covered = width.saturating_sub(1);
        }
        if (i + 1) % w == 0 {
            lines.push(std::mem::take(&mut line));
            covered = 0;
        }
    }
    if let Some(head) = lines.first_mut() {
        if let Some(at) = head.find(|c: char| ('\u{2800}'..='\u{28ff}').contains(&c)) {
            let end = head[at..].find("   ").map_or(head.len(), |e| at + e);
            let blank = " ".repeat(head[at..end].chars().count());
            head.replace_range(at..end, &blank);
        }
    }
    lines.join("\n")
}

/// A screen with what differs between two sessions of one process for no reason of the roads: a
/// kitty picture's id (a process-wide counter, carried in the placeholder's colour) and a jj change
/// id (random per workspace).
fn kind_unnumbered(screen: &str) -> String {
    let mut out = String::with_capacity(screen.len());
    let mut rest = screen;
    const ID: &str = "\u{1b}[38;2;";
    while let Some(at) = rest.find(ID) {
        out.push_str(&rest[..at]);
        let tail = &rest[at + ID.len()..];
        let end = tail.find('m').map_or(tail.len(), |e| e + 1);
        out.push_str("<colour>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out.lines()
        .map(|line| match line.find("  @ ") {
            Some(at) => {
                let id = at + 4;
                let end = line[id..].find(' ').map_or(line.len(), |e| id + e);
                format!("{}<change>{}", &line[..id], &line[end..])
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The live work of a tab's view that `App` holds: what the screen in front would draw or ask
/// for. A tab that is away holds none of it (covering is leaving, `leave_tab_view`): neither
/// after it is left nor after a result for it is delivered. Still pictures of the inline cache
/// stay (they cost nothing while they are not drawn) and are not listed.
fn kind_held(app: &App) -> Vec<&'static str> {
    let mut held = Vec::new();
    let mut note = |on: bool, what: &'static str| {
        if on {
            held.push(what);
        }
    };
    note(app.is_gif_active(), "a full-screen GIF plays");
    note(app.gif_poll_timeout().is_some(), "the full-screen GIF asks for wake-ups");
    note(app.md_gif_poll_timeout().is_some(), "an inline GIF asks for wake-ups");
    note(app.is_image_preview(), "a picture is shown");
    note(app.is_media_loading(), "a media load is awaited");
    note(app.kitty_build_pending(), "a kitty build is awaited");
    note(app.plugin_layout_for_test().is_some(), "a plugin show is held");
    note(app.md_cache_width_for_test().is_some(), "a Markdown decoration is held");
    note(app.md_diff_pending_for_current(), "a block diff is awaited");
    note(app.diff_target_resolved_for_test(), "a diff target answer is held");
    note(
        !app.md_image_cache_media_diff_keys_for_test().is_empty(),
        "media diff pictures are held",
    );
    held
}

/// A session with two tabs on the same root, the first in front: both roads start from it (the
/// tab road switches to the second tab, the project road covers the first).
fn kind_sim(repo: &std::path::Path, sim: Sim) -> Sim {
    let _ = repo;
    let mut sim = sim;
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    sim.app.tab_cycle(-1);
    drain(&mut sim);
    assert_eq!(sim.app.active_tab_index(), 0, "the first tab is in front");
    sim
}

fn kind_plain(repo: &std::path::Path) -> Sim {
    kind_sim(repo, Sim::with_config_sized(repo, config("git", "en"), 120, 30))
}

fn kind_media(repo: &std::path::Path) -> Sim {
    kind_sim(repo, Sim::with_config_sized(repo, config("git", "en"), 120, 30).with_media())
}

/// Applies every result `rx` still owes (what the return asked), until it is quiet.
fn kind_settle_rx<R>(sim: &mut Sim, rx: &std::sync::mpsc::Receiver<R>, mut apply: impl FnMut(&mut App, R) -> bool) {
    while let Ok(r) = rx.recv_timeout(std::time::Duration::from_millis(1500)) {
        apply(&mut sim.app, r);
        sim.draw();
    }
}

type Held<R> = std::cell::RefCell<Option<R>>;

fn held<R>(r: R) -> Held<R> {
    std::cell::RefCell::new(Some(r))
}

fn take<R>(h: &Held<R>) -> R {
    h.borrow_mut().take().expect("the held result is delivered once")
}

/// Runs one kind on both roads. `make` builds a session whose result is owed with the tab in
/// front (and holds it); `deliver` hands it to the loop's apply while the tab is away; `settle`
/// lands what the return asks. The screen in front is unchanged by the delivery, and the screen
/// after the return is the same on both roads.
fn kind_parity<T>(
    kind: &str,
    make: impl Fn(&str) -> (Sim, crate::test_support::TmpDir, T),
    deliver: impl Fn(&mut Sim, &T),
    settle: impl Fn(&mut Sim, &T),
) {
    kind_parity_with(kind, KindFront::Unchanged, &[], make, deliver, settle);
}

/// What the delivery may do to the screen in front on the tab road.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KindFront {
    Unchanged,
    /// The kind acts on what the tab in front shows (follow is App-level and moves whatever tab is
    /// shown; a file operation and a status scan change the files and the marks the other tab, on
    /// the same root, lists): on the tab road the other tab may change. Under a project screen
    /// nothing changes either way.
    MayChangeOnTheTabRoad,
}

/// `same` lists texts the two roads write differently on purpose (`(cover road's, tab road's)`),
/// compared as one.
fn kind_parity_with<T>(
    kind: &str,
    front_rule: KindFront,
    same: &[(&str, &str)],
    make: impl Fn(&str) -> (Sim, crate::test_support::TmpDir, T),
    deliver: impl Fn(&mut Sim, &T),
    settle: impl Fn(&mut Sim, &T),
) {
    let mut after = Vec::new();
    for road in [KindRoad::Cover, KindRoad::TabSwitch] {
        // Road tags of one length: the repo name is in the frame's title, whose rule fills the rest.
        let tag = match road {
            KindRoad::Cover => "cov",
            KindRoad::TabSwitch => "tab",
        };
        let name = format!("vcs_kind_{}_{tag}", kind.replace(' ', "_"));
        let (mut sim, repo, extra) = make(&name);
        let repo_name = repo.file_name().unwrap().to_string_lossy().into_owned();
        kind_away(&mut sim, road, kind);
        let front = kind_front(&mut sim);
        assert_eq!(
            kind_held(&sim.app),
            Vec::<&str>::new(),
            "{kind} {road:?}: the tab that is away still holds its view's live work"
        );
        deliver(&mut sim, &extra);
        drain(&mut sim);
        if road == KindRoad::Cover || front_rule == KindFront::Unchanged {
            assert_eq!(
                kind_held(&sim.app),
                Vec::<&str>::new(),
                "{kind} {road:?}: the result delivered while the tab is away was applied to its view"
            );
            assert_eq!(
                kind_front(&mut sim),
                front,
                "{kind} {road:?}: the result delivered while the tab is away changed the screen in front"
            );
        }
        if road == KindRoad::Cover {
            assert!(sim.app.project_active(), "{kind}: the project screen is still in front");
        } else {
            assert_eq!(sim.app.active_tab_index(), 1, "{kind}: the other tab is still in front");
        }
        kind_back(&mut sim, road, kind);
        settle(&mut sim, &extra);
        drain(&mut sim);
        let mut screen = kind_unnumbered(&kind_front(&mut sim).replace(&repo_name, "<repo>"));
        for (cover, tab) in same {
            screen = screen.replace(if road == KindRoad::Cover { cover } else { tab }, "<same>");
        }
        after.push(screen);
    }
    assert_eq!(
        after[0], after[1],
        "{kind}: the screen after leaving the project screen is not the tab switch's screen"
    );
}

const KIND_MD_IMAGE: &str = "# Doc\n\nbefore\n\n![pic](pic.png)\n\nafter\n";

#[test]
fn vcs_kind_image_resize_delivered_while_away() {
    if isolated("vcs_kind_image_resize_delivered_while_away") {
        return;
    }
    kind_parity(
        "image resize",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            crate::test_support::write_solid_png(&repo.join("pic.png"), 40, 20, [10, 210, 10]);
            let mut sim = kind_media(&repo);
            sim.select("pic.png");
            sim.enter();
            sim.settle_inline_images("open", DRAIN_WAIT);
            let req = sim
                .resize_rx
                .as_mut()
                .unwrap()
                .try_recv()
                .expect("premise: the picture asked a resize");
            (sim, repo, held(req))
        },
        |sim, h| {
            let req = take(h);
            sim.app.apply_image_resize(req.resize_encode());
        },
        |sim, _| {
            sim.settle_inline_images("back", DRAIN_WAIT);
            sim.answer_image_resizes();
        },
    );
}

#[test]
fn vcs_kind_media_load_delivered_while_away() {
    if isolated("vcs_kind_media_load_delivered_while_away") {
        return;
    }
    kind_parity(
        "media load",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::write(
                repo.join("pic.svg"),
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\">\
                 <rect width=\"80\" height=\"40\" fill=\"#10d210\"/></svg>\n",
            )
            .unwrap();
            let mut sim = kind_media(&repo);
            sim.select("pic.svg");
            sim.enter();
            assert!(sim.app.is_media_loading(), "premise: the SVG is on the worker");
            let res = sim.media_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the worker answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_media(take(h));
        },
        |sim, _| sim.settle_inline_images("back", DRAIN_WAIT),
    );
}

#[test]
fn vcs_kind_kitty_build_delivered_while_away() {
    if isolated("vcs_kind_kitty_build_delivered_while_away") {
        return;
    }
    kind_parity(
        "kitty build",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            crate::test_support::write_solid_png(&repo.join("pic.png"), 400, 200, [10, 210, 10]);
            let mut sim = kind_sim(
                &repo,
                Sim::with_config_sized(&repo, config("git", "en"), 120, 30).with_media_kitty(),
            );
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_kitty_loader(tx);
            sim.select("pic.png");
            sim.enter();
            sim.settle_inline_images("open", DRAIN_WAIT);
            sim.app.tab.image_view.zoom = 4.0;
            sim.draw();
            assert!(sim.app.kitty_build_pending(), "premise: the zoomed picture is on the worker");
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the kitty worker answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            sim.app.apply_kitty(take(h));
        },
        |sim, (_, rx)| {
            sim.settle_inline_images("back", DRAIN_WAIT);
            while sim.app.kitty_build_pending() {
                let res = rx.recv_timeout(DRAIN_WAIT).expect("the kitty worker answers");
                sim.app.apply_kitty(res);
                sim.draw();
            }
        },
    );
}

#[test]
fn vcs_kind_md_image_decode_delivered_while_away() {
    if isolated("vcs_kind_md_image_decode_delivered_while_away") {
        return;
    }
    kind_parity(
        "md image decode",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            crate::test_support::write_solid_png(&repo.join("pic.png"), 40, 20, [10, 210, 10]);
            std::fs::write(repo.join("doc.md"), KIND_MD_IMAGE).unwrap();
            let mut sim = kind_media(&repo);
            sim.select("doc.md");
            sim.enter();
            sim.await_media("open");
            sim.draw();
            let res = sim.md_img_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the decode answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_md_image(take(h));
        },
        |sim, _| sim.settle_inline_images("back", DRAIN_WAIT),
    );
}

#[test]
fn vcs_kind_md_image_encode_delivered_while_away() {
    if isolated("vcs_kind_md_image_encode_delivered_while_away") {
        return;
    }
    kind_parity(
        "md image encode",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            crate::test_support::write_solid_png(&repo.join("pic.png"), 40, 20, [10, 210, 10]);
            std::fs::write(repo.join("doc.md"), KIND_MD_IMAGE).unwrap();
            let mut sim = kind_media(&repo);
            sim.select("doc.md");
            sim.enter();
            sim.drain_md_images();
            assert!(sim.app.md_encode_in_flight(), "premise: the encode is out");
            let res = sim.md_enc_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the encode answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_md_encode(take(h));
        },
        |sim, _| sim.settle_inline_images("back", DRAIN_WAIT),
    );
}

#[test]
fn vcs_kind_md_remote_fetch_delivered_while_away() {
    if isolated("vcs_kind_md_remote_fetch_delivered_while_away") {
        return;
    }
    kind_parity(
        "md remote fetch",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            // A port nothing listens on: the fetch fails at once (no network is reached).
            std::fs::write(repo.join("doc.md"), "# Doc\n\n![r](http://127.0.0.1:9/x.png)\n\nafter\n").unwrap();
            // The fetch writes under this sandbox, never the real cache; its guard is held with
            // the result for the whole road.
            let cache = crate::test_support::unique_tmp(&format!("{name}_cache"));
            crate::test_support::set_test_cache_root(cache.to_path_buf());
            let mut sim = kind_media(&repo);
            sim.select("doc.md");
            sim.enter();
            sim.await_media("open");
            sim.draw();
            let res = sim.md_remote_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the fetch answers");
            (sim, repo, (held(res), cache))
        },
        |sim, (h, _)| {
            sim.app.apply_remote_fetch(take(h));
        },
        |sim, _| {
            sim.settle_inline_images("back", DRAIN_WAIT);
            while let Ok(res) = sim.md_remote_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_millis(1500)) {
                sim.app.apply_remote_fetch(res);
                sim.draw();
            }
        },
    );
}

#[test]
fn vcs_kind_ignored_scan_delivered_while_away() {
    if isolated("vcs_kind_ignored_scan_delivered_while_away") {
        return;
    }
    kind_parity(
        "ignored scan",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::write(repo.join(".gitignore"), "skip.txt\n").unwrap();
            std::fs::write(repo.join("skip.txt"), "s\n").unwrap();
            let mut sim = kind_plain(&repo);
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_git_loader(tx);
            sim.app.refresh_fs(true).unwrap();
            sim.draw();
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the ignored scan answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            sim.app.apply_ignored(take(h));
        },
        |sim, (_, rx)| kind_settle_rx(sim, rx, |app, r| app.apply_ignored(r)),
    );
}

#[test]
fn vcs_kind_status_scan_delivered_while_away() {
    if isolated("vcs_kind_status_scan_delivered_while_away") {
        return;
    }
    kind_parity_with(
        "status scan",
        KindFront::MayChangeOnTheTabRoad,
        &[],
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let mut sim = kind_plain(&repo);
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_status_loader(tx);
            std::fs::write(repo.join("new-file.txt"), "n\n").unwrap();
            sim.app.refresh_fs(false).unwrap();
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the status scan answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            sim.app.apply_statuses(take(h));
        },
        |sim, (_, rx)| kind_settle_rx(sim, rx, |app, r| app.apply_statuses(r)),
    );
}

#[test]
fn vcs_kind_follow_baseline_delivered_while_away() {
    if isolated("vcs_kind_follow_baseline_delivered_while_away") {
        return;
    }
    // The follow worker runs only for a jj workspace (git captures the baseline at once).
    if !crate::vcs::jj::available() {
        return;
    }
    kind_parity(
        "follow baseline",
        |name| {
            let repo = crate::test_support::jj_scratch_seeded(name).expect("a jj workspace");
            let mut sim = kind_sim(&repo, Sim::with_config_sized(&repo, config("jj", "en"), 120, 30));
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_follow_loader(tx);
            sim.app.toggle_follow();
            assert!(sim.app.follow_work_pending(), "premise: the follow baseline is on the worker");
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the follow worker answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            sim.app.apply_follow(take(h));
        },
        |sim, (_, rx)| {
            while sim.app.follow_work_pending() {
                let res = rx.recv_timeout(DRAIN_WAIT).expect("the follow worker answers");
                sim.app.apply_follow(res);
                sim.draw();
            }
        },
    );
}

#[test]
fn vcs_kind_md_block_diff_delivered_while_away() {
    if isolated("vcs_kind_md_block_diff_delivered_while_away") {
        return;
    }
    kind_parity(
        "md block diff",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let doc = overlay_tracked_change(
                &repo,
                "doc.md",
                b"# Title\n\nold paragraph\n",
                b"# Title\n\nnew paragraph\n\nadded block\n",
            );
            let mut sim = kind_sim(
                &repo,
                Sim::with_config_sized(&repo, config("git", "en"), 120, 30).with_async_md_diff(),
            );
            sim.app.open_git_diff(&doc);
            assert!(sim.app.md_diff_pending_for_current(), "premise: the block diff is out");
            let res = sim.md_diff_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the block diff answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_md_diff(take(h));
        },
        |sim, _| {
            sim.draw();
            if sim.app.md_diff_pending_for_current() {
                sim.drain_md_diff();
            }
        },
    );
}

#[test]
fn vcs_kind_media_diff_delivered_while_away() {
    if isolated("vcs_kind_media_diff_delivered_while_away") {
        return;
    }
    kind_parity(
        "media diff",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let png = repo.join("pic.png");
            crate::test_support::write_solid_png(&png, 40, 20, [10, 210, 10]);
            crate::test_support::run_git(&repo, &["add", "pic.png"]);
            crate::test_support::run_git(&repo, &["commit", "-qm", "pic", "--", "pic.png"]);
            crate::test_support::write_solid_png(&png, 40, 20, [210, 10, 10]);
            let mut sim = kind_plain(&repo);
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_media_diff_loader(tx);
            sim.app.open_git_diff(&png);
            sim.draw();
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the media diff answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            sim.app.apply_media_diff(take(h));
        },
        |sim, (_, rx)| kind_settle_rx(sim, rx, |app, r| app.apply_media_diff(r)),
    );
}

#[test]
fn vcs_kind_file_operation_delivered_while_away() {
    if isolated("vcs_kind_file_operation_delivered_while_away") {
        return;
    }
    kind_parity_with(
        "file operation",
        KindFront::MayChangeOnTheTabRoad,
        &[],
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::create_dir_all(repo.join("dest")).unwrap();
            std::fs::write(repo.join("src-file.txt"), "s\n").unwrap();
            let mut sim = kind_sim(
                &repo,
                Sim::with_config_sized(&repo, config("git", "en"), 120, 30).with_async_file_ops(),
            );
            sim.select("src-file.txt");
            sim.key(' ');
            sim.key('c');
            sim.select("dest");
            sim.key(' ');
            sim.key('p');
            let res = sim.fileop_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the copy answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_file_op(take(h));
        },
        |_, _| {},
    );
}

#[test]
fn vcs_kind_office_launch_delivered_while_away() {
    if isolated("vcs_kind_office_launch_delivered_while_away") {
        return;
    }
    kind_parity(
        "office launch",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::write(repo.join("book.xlsx"), b"not a real book").unwrap();
            let mut sim = kind_plain(&repo);
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_office_opener(tx);
            // A recording runner: no process starts.
            let runner: konoma_plugin::launch::Runner =
                std::sync::Arc::new(|_: &konoma_plugin::launch::Attempt| Ok(Some(0)));
            sim.app.set_office_runner(runner);
            sim.select("book.xlsx");
            sim.key('e');
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the launch chain answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            // Through the run loop's own function.
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(take(h)).unwrap();
            crate::apply_office_results(&mut sim.app, &rx);
        },
        |_, _| {},
    );
}

#[test]
fn vcs_kind_git_write_delivered_while_away() {
    if isolated("vcs_kind_git_write_delivered_while_away") {
        return;
    }
    kind_parity(
        "git write",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let mut sim = kind_sim(
                &repo,
                Sim::with_config_sized(&repo, config("git", "en"), 120, 30).with_async_git_ops(),
            );
            sim.select("b-unstaged.txt");
            sim.key('d');
            assert!(sim.app.is_git_diff_preview(), "premise: d opens the file diff");
            sim.key('x');
            sim.key('y');
            let res = sim.gitop_rx.as_ref().unwrap().recv_timeout(DRAIN_WAIT).expect("the git write answers");
            (sim, repo, held(res))
        },
        |sim, h| {
            sim.app.apply_git_op(take(h));
        },
        |_, _| {},
    );
}

#[test]
fn vcs_kind_filter_pool_delivered_while_away() {
    if isolated("vcs_kind_filter_pool_delivered_while_away") {
        return;
    }
    kind_parity(
        "filter pool",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let mut sim = kind_plain(&repo);
            sim.key('/');
            sim.key('a');
            sim.enter();
            assert!(sim.app.filter_query().is_some(), "premise: the filter is on");
            let (tx, rx) = std::sync::mpsc::channel();
            sim.app.attach_filter_pool_loader(tx);
            std::fs::write(repo.join("a-new.txt"), "n\n").unwrap();
            sim.app.refresh_fs(true).unwrap();
            assert!(sim.app.filter_pool_scan_in_flight(), "premise: the pool scan is out");
            let res = rx.recv_timeout(DRAIN_WAIT).expect("the pool scan answers");
            (sim, repo, (held(res), rx))
        },
        |sim, (h, _)| {
            // The scan was asked for the tab that is away: it lands nowhere (the screen in front
            // keeps its own pool), and the return scans again.
            let pool = sim.app.filter_pool_names_for_test();
            assert!(
                !sim.app.apply_filter_pool(take(h)),
                "filter pool: the scan of the tab that is away was applied"
            );
            assert_eq!(sim.app.filter_pool_names_for_test(), pool, "filter pool: the pool in front changed");
        },
        |sim, (_, rx)| kind_settle_rx(sim, rx, |app, r| app.apply_filter_pool(r)),
    );
}

#[test]
fn vcs_kind_highlight_warm_delivered_while_away() {
    if isolated("vcs_kind_highlight_warm_delivered_while_away") {
        return;
    }
    // A grammar the code plugin cannot style at once stays cold, so the run loop warms it on a
    // thread (a rule makes an unknown extension a code file, as the plugin's own unit test does). A
    // grammar is warm for the rest of the process: each road has an extension of its own.
    kind_parity_with(
        "highlight warm",
        KindFront::Unchanged,
        &[("code.zzcoldcov", "code.zzcoldtab")],
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            let ext = if name.ends_with("_cov") { "zzcoldcov" } else { "zzcoldtab" };
            let file = format!("code.{ext}");
            std::fs::write(repo.join(&file), "x = 1\nprint(x)\n").unwrap();
            let mut cfg = config("git", "en");
            let rules: Config =
                toml::from_str(&format!("[[preview.rules]]\nglob = \"*.{ext}\"\nbuiltin = \"code\"\n")).unwrap();
            cfg.preview.rules.extend(rules.preview.rules);
            let mut sim = kind_sim(&repo, Sim::with_config_sized(&repo, cfg, 120, 30));
            let file = file.as_str();
            sim.select(file);
            sim.enter();
            let (ext, path) = sim.app.take_warm_job().unwrap_or_else(|| {
                panic!(
                    "premise: the {file} grammar is warmed on a thread (warm already: {}, pending: {})",
                    crate::preview::code::is_ext_warm(ext),
                    sim.app.is_highlight_pending()
                )
            });
            // What the run loop's thread does; its `()` is the result delivered below.
            crate::preview::code::warm_file(&ext, &path);
            (sim, repo, ())
        },
        |sim, ()| sim.app.clear_highlight_pending(),
        |_, _| {},
    );
}

#[test]
fn vcs_kind_watch_event_delivered_while_away() {
    if isolated("vcs_kind_watch_event_delivered_while_away") {
        return;
    }
    kind_parity(
        "watch event",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::write(repo.join("lines.txt"), LINES_TXT).unwrap();
            let mut sim = kind_plain(&repo);
            sim.select("lines.txt");
            sim.enter();
            let path = repo.join("lines.txt");
            (sim, repo, path)
        },
        |sim, path| {
            std::fs::write(path, "rewritten while away\n").unwrap();
            sim.app.refresh_fs_watched(false, std::slice::from_ref(path));
        },
        |_, _| {},
    );
}

#[test]
fn vcs_kind_follow_jump_delivered_while_away() {
    if isolated("vcs_kind_follow_jump_delivered_while_away") {
        return;
    }
    kind_parity_with(
        "follow jump",
        KindFront::MayChangeOnTheTabRoad,
        &[],
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            std::fs::write(repo.join("lines.txt"), LINES_TXT).unwrap();
            let mut sim = kind_plain(&repo);
            sim.select("lines.txt");
            sim.enter();
            sim.app.toggle_follow();
            assert!(sim.app.follow_enabled(), "premise: follow is on");
            let path = repo.join("a-followed.txt");
            (sim, repo, path)
        },
        |sim, path| {
            // The run loop's dwell: a noted change, then the jump once the dwell is over.
            std::fs::write(path, "followed\n").unwrap();
            sim.app.refresh_fs_watched(false, std::slice::from_ref(path));
            if sim.app.follow_note_change(path) {
                sim.app.follow_jump(path);
            }
        },
        |_, _| {},
    );
}

#[test]
fn vcs_kind_gif_timer_delivered_while_away() {
    if isolated("vcs_kind_gif_timer_delivered_while_away") {
        return;
    }
    kind_parity(
        "gif timer",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            write_animated_gif(&repo.join("anim.gif"), &[[220, 20, 20], [20, 220, 20], [20, 20, 220]]);
            let mut sim = kind_media(&repo);
            sim.select("anim.gif");
            sim.enter();
            sim.settle_inline_images("open", DRAIN_WAIT);
            assert!(sim.app.is_gif_active(), "premise: the GIF plays");
            (sim, repo, ())
        },
        |sim, ()| {
            // Every timer of the loop, past any frame's time: nothing of the tab that is away
            // advances (its GIF went with the leave), so nothing asks to be drawn.
            let due = crate::animation_timers_due(&mut sim.app, std::time::Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(300));
            let due_later = crate::animation_timers_due(&mut sim.app, std::time::Instant::now());
            assert!(!due && !due_later, "gif timer: a timer of the tab that is away fired");
        },
        |sim, ()| sim.settle_inline_images("back", DRAIN_WAIT),
    );
}

#[test]
fn vcs_kind_md_gif_timer_delivered_while_away() {
    if isolated("vcs_kind_md_gif_timer_delivered_while_away") {
        return;
    }
    kind_parity(
        "md gif timer",
        |name| {
            let repo = super::vcs_changes::git_repo(name, true);
            write_animated_gif(&repo.join("anim.gif"), &[[220, 20, 20], [20, 220, 20], [20, 20, 220]]);
            std::fs::write(repo.join("anim.md"), "before\n\n![anim](anim.gif)\n\nafter\n").unwrap();
            let mut sim = kind_media(&repo);
            sim.select("anim.md");
            sim.enter();
            sim.drain_md_images();
            sim.drain_md_encodes();
            assert!(sim.app.md_gif_poll_timeout().is_some(), "premise: the GIF asks for wake-ups");
            (sim, repo, ())
        },
        |sim, ()| {
            // Nothing of the tab that is away advances (its animated entries went with the
            // leave), so nothing asks to be drawn.
            let due = crate::animation_timers_due(&mut sim.app, std::time::Instant::now());
            sim.app.age_md_gif_frames_for_test();
            let due_later = crate::animation_timers_due(&mut sim.app, std::time::Instant::now());
            assert!(!due && !due_later, "md gif timer: a timer of the tab that is away fired");
        },
        |sim, ()| sim.settle_inline_images("back", DRAIN_WAIT),
    );
}
