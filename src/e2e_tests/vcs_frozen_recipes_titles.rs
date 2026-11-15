//! Recipes of the old Changes-title tests in `vcs_changes.rs`. Each frame is the body of the
//! screen plus the title part of the top edge (the old `compare_body` compared both).

use super::super::super::vcs_changes::plugin_config;
use super::super::super::*;
use super::super::Ctx;
use super::{git_repo_at, stable_root};

/// The top edge up to and including the first filler cell: the corner, the title, the filter and
/// the shown count. The right end of the edge differs between the old hub (filler) and the
/// project host (screen tabs), so it is not part of the frame; the end corner is frozen apart.
fn title_edge(c: &mut Ctx, label: &str, width: u16) {
    let sim = c.sim();
    let buffer = sim
        .term
        .draw(|frame| ui::render(frame, &mut sim.app))
        .unwrap()
        .buffer
        .clone();
    let mut end = 1;
    while end < width && buffer[(end, 1)].symbol() != "─" {
        end += 1;
    }
    assert!(end + 1 < width, "{label}: no filler cell after the title");
    c.region(&format!("{label} title"), 0..end + 1, 1..2, (0, 0), (0, 0));
    c.region(
        &format!("{label} corner"),
        width - 1..width,
        1..2,
        (0, 0),
        (0, 0),
    );
}

fn type_query(c: &mut Ctx, query: &str) {
    if query.is_empty() {
        return;
    }
    c.key('/');
    for key in query.chars() {
        c.raw_key(key);
    }
    c.settle();
    c.enter();
}

fn open_changes(c: &mut Ctx, root: &std::path::Path, lang: &str, icons: bool, width: u16) {
    let mut cfg = plugin_config("git", "vcs_changes_titles", lang);
    cfg.ui.icons = icons;
    c.open(root, "git", cfg, width, 12);
}

pub(super) fn matrix(c: &mut Ctx) {
    use crate::test_support::run_git;
    for state in ["branch", "detached"] {
        let (_guard, dir) = stable_root("vcs_changes_titles");
        git_repo_at(&dir, true);
        if state == "detached" {
            run_git(&dir, &["checkout", "-q", "--detach"]);
        }
        for icons in [false, true] {
            for lang in ["en", "ja"] {
                for query in ["", "txt", "a-", "zzz-no-hit"] {
                    open_changes(c, &dir, lang, icons, 90);
                    type_query(c, query);
                    let label = format!("state={state} icons={icons} lang={lang} q={query:?}");
                    title_edge(c, &label, 90);
                    c.body(&label, 90, 12);
                }
            }
        }
    }
}

pub(super) fn unborn(c: &mut Ctx) {
    let (_guard, dir) = stable_root("vcs_changes_title_unborn");
    crate::test_support::init_git_repo(&dir);
    std::fs::write(dir.join("new.txt"), "x\n").unwrap();
    for icons in [false, true] {
        open_changes(c, &dir, "en", icons, 60);
        let label = format!("icons={icons}");
        title_edge(c, &label, 60);
        c.body(&label, 60, 10);
    }
}

/// Replace HEAD by a commit with the same tree and a fixed date, so the Log body (hash and time)
/// is the same on every run. The index and the work tree are untouched.
fn pin_head_date(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_DATE", "1577836800 +0000")
            .env("GIT_COMMITTER_DATE", "1577836800 +0000")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let commit = git(&["commit-tree", "HEAD^{tree}", "-m", "seed"]);
    git(&["update-ref", "HEAD", &commit]);
}

pub(super) fn words(c: &mut Ctx) {
    let (_guard, dir) = stable_root("vcs_changes_title_words");
    git_repo_at(&dir, true);
    pin_head_date(&dir);
    for icons in [false, true] {
        for lang in ["en", "ja"] {
            open_changes(c, &dir, lang, icons, 80);
            let label = format!("icons={icons} lang={lang}");
            title_edge(c, &format!("{label} changes"), 80);
            c.body(&format!("{label} changes"), 80, 12);
            c.tab();
            title_edge(c, &format!("{label} log"), 80);
            c.body(&format!("{label} log"), 80, 12);
        }
    }
}
