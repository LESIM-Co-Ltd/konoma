//! Linked worktrees through the project host.

use konoma_vcs_core::git2;
use std::path::{Path, PathBuf};

pub(super) fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub(super) fn commit(root: &Path, message: &str, time: i64) {
    let repo = git2::Repository::open(root).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(time, 0),
    )
    .unwrap();
    let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap();
}

pub(super) fn seed(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = dir.join("main");
    git2::Repository::init(&root).unwrap();
    git(&root, &["branch", "-m", "main"]);
    std::fs::write(root.join("file.txt"), "base\n").unwrap();
    commit(&root, "seed", 1_577_836_800);
    let linked = dir.join("linked");
    let detached = dir.join("detached");
    git(
        &root,
        &["worktree", "add", "-qb", "日本語", linked.to_str().unwrap()],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
        ],
    );
    let missing = dir.join("prunable");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-qb",
            "missing",
            missing.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&missing).unwrap();
    git(
        &root,
        &[
            "worktree",
            "lock",
            "--reason",
            "fixture lock",
            linked.to_str().unwrap(),
        ],
    );
    (root, linked, detached)
}

pub(super) fn seed_bare(dir: &Path) -> (PathBuf, PathBuf) {
    git(dir, &["clone", "-q", "--bare", "main", "bare"]);
    let bare = dir.join("bare");
    let linked = dir.join("bare-linked");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    (bare, linked)
}
