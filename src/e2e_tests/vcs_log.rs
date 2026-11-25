//! Linear log bodies through the project host.

use super::vcs_changes::{drain, plugin_config, project_sim};
use super::*;
use crate::test_support::{init_git_repo, TmpDir};
use konoma_vcs_core::git2;

#[path = "vcs_graph.rs"]
pub(super) mod graph;
#[path = "vcs_worktrees.rs"]
mod worktrees;

pub(super) fn seed_worktrees(
    dir: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    worktrees::seed(dir)
}

pub(super) fn worktree_git(root: &std::path::Path, args: &[&str]) -> String {
    worktrees::git(root, args)
}

pub(super) fn seed_bare_worktrees(
    dir: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    worktrees::seed_bare(dir)
}

pub(super) fn commit_worktree(root: &std::path::Path) {
    worktrees::commit(root, "feature", 1_577_837_000);
}

pub(super) fn graph_picker_fixture() -> TmpDir {
    graph::picker_fixture()
}
pub(super) fn graph_fixture() -> TmpDir {
    graph::fixture()
}
pub(super) fn graph_jj_fixture() -> TmpDir {
    graph::jj_graph_fixture()
}

pub(super) fn commit(root: &std::path::Path, i: usize) {
    let repo = git2::Repository::open(root).unwrap();
    let sig = git2::Signature::new(
        "作者 Author",
        "fixture@example.invalid",
        &git2::Time::new(1_577_836_800 + i as i64 * 60, 0),
    )
    .unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
    let subject = if i.is_multiple_of(2) {
        format!("{i} 日本語の長い件名 {} Summary", "長文 abc ".repeat(12))
    } else {
        format!("{i} 日本語 Summary")
    };
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &subject,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap();
}

pub(super) fn repo(count: usize) -> TmpDir {
    let dir = crate::test_support::unique_tmp("vcs_log_parity");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    for i in 0..count {
        commit(&dir, i);
    }
    dir.into_canonical().unwrap()
}

pub(super) fn detail_repo() -> TmpDir {
    let dir = repo(1);
    for (path, text) in [
        ("main.rs", "let value = 123;"),
        ("data.json", "{\"value\":123}"),
    ] {
        let body = (0..70)
            .map(|i| format!("{text} // {i} 日本語 {}\n", "long ".repeat(45)))
            .collect::<String>();
        std::fs::write(dir.join(path), body).unwrap();
    }
    let repository = git2::Repository::open(&dir).unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(std::path::Path::new("main.rs")).unwrap();
    index.add_path(std::path::Path::new("data.json")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    for (i, message) in [
        "Subject 日本語\n\nBody one\n\nBody three",
        "Empty subject\n\nEmpty body",
    ]
    .into_iter()
    .enumerate()
    {
        let message = if i == 0 {
            format!("{message} {}", "long message ".repeat(45))
        } else {
            message.to_string()
        };
        let sig = git2::Signature::new(
            "作者 Author",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_860 + i as i64 * 60, 0),
        )
        .unwrap();
        let parent = repository.head().unwrap().peel_to_commit().unwrap();
        repository
            .commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&parent])
            .unwrap();
    }
    dir
}

pub(super) fn empty_subject_repo() -> TmpDir {
    let dir = repo(1);
    empty_subject_commit(&dir);
    dir
}

pub(super) fn empty_subject_commit(dir: &std::path::Path) {
    let repository = git2::Repository::open(dir).unwrap();
    let parent = repository.head().unwrap().peel_to_commit().unwrap();
    let signature = parent.author();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "",
            &parent.tree().unwrap(),
            &[&parent],
        )
        .unwrap();
}

pub(super) fn remove_head_object(dir: &std::path::Path) {
    let id = git2::Repository::open(dir)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    remove_commit_object(dir, &id);
}

pub(super) fn remove_commit_object(dir: &std::path::Path, id: &str) {
    let gitdir = if dir.join(".git").exists() {
        dir.join(".git")
    } else {
        dir.join(".jj/repo/store/git")
    };
    std::fs::remove_file(gitdir.join("objects").join(&id[..2]).join(&id[2..])).unwrap();
}

thread_local! {
    /// The frozen frames show jj change ids; while this is `Some(n)`, every `jj()` call seeds jj's
    /// id generator with `n` and counts up, so a fixture built twice has the same ids.
    static JJ_ID_SEED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(super) fn seed_jj_ids(start: Option<u64>) {
    JJ_ID_SEED.with(|s| s.set(start));
}

pub(super) fn jj(root: &std::path::Path, args: &[&str]) {
    let seed = JJ_ID_SEED.with(|s| {
        let current = s.get();
        s.set(current.map(|n| n + 1));
        current
    });
    let mut command = std::process::Command::new("jj");
    command
        .current_dir(root)
        .env("HOME", root)
        .env("JJ_USER", "作者 Author")
        .env("JJ_EMAIL", "fixture@example.invalid")
        .args([
            "--config",
            "debug.commit-timestamp=\"2020-01-01T00:20:00Z\"",
            "--config",
            "debug.operation-timestamp=\"2020-01-01T00:20:00Z\"",
        ]);
    if let Some(seed) = seed {
        command.args(["--config", &format!("debug.randomness-seed={seed}")]);
    }
    let output = command.args(args).output().unwrap();
    assert!(
        output.status.success(),
        "jj {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Preference;
impl Preference {
    fn pin(id: &str) -> Self {
        crate::vcs::set_preference_for_test(Some(if id == "git" {
            crate::vcs::Preference::Git
        } else {
            crate::vcs::Preference::Jj
        }));
        Self
    }
}
impl Drop for Preference {
    fn drop(&mut self) {
        crate::vcs::set_preference_for_test(None);
    }
}

pub(super) fn jj_repo(colocated: bool) -> TmpDir {
    let dir = if colocated {
        repo(5)
    } else {
        let dir = crate::test_support::unique_tmp("vcs_log_jj");
        std::fs::create_dir_all(&dir).unwrap();
        dir.into_canonical().unwrap()
    };
    jj(
        &dir,
        &[
            "git",
            "init",
            if colocated {
                "--colocate"
            } else {
                "--no-colocate"
            },
            ".",
        ],
    );
    jj(&dir, &["describe", "-m", "jj 専用 Summary"]);
    jj(&dir, &["new", "-m", "次の日本語 Summary"]);
    dir
}
