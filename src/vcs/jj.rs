//! The jj (Jujutsu) backend.
//!
//! **Everything here reads. konoma never writes to a jj repository.** That is not a convention but
//! a property of [`args`]: every invocation carries `--ignore-working-copy`, and a test holds it
//! there. It matters because in jj the working copy *is* a commit — any command without that flag
//! first snapshots the working directory into a new commit, so a tool that re-reads status on every
//! file-watch event would feed its own writes back into the watcher.
//!
//! The flag has a price: it answers from the last snapshot somebody else took, so `jj diff` alone
//! goes stale the moment a file changes. konoma closes that gap itself — it walks the tree anyway,
//! so it can see which files are newer than the last snapshot and ask jj only about those.
//!
//! Nothing here reads jj's on-disk layout. jj is pre-1.0 and says its storage format will change
//! before then; a wrong guess about that layout would not fail loudly, it would quietly show the
//! wrong diff. Only the documented CLI is used.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::git::FileStatus;

/// The flag that keeps every read from writing. See the module docs.
const IGNORE_WORKING_COPY: &str = "--ignore-working-copy";

/// How many files konoma will read out of jj in one scan to tell "touched" from "actually changed".
/// Only files that changed *after* the last snapshot need it, so this is normally a handful; the cap
/// keeps a bulk rewrite (an agent reformatting a tree) from turning one scan into thousands of
/// processes. Anything past the cap keeps the optimistic answer, which errs towards showing a
/// marker rather than hiding one.
const CONFIRM_CAP: usize = 256;

/// Command line konoma runs jj with. **Never build one any other way** — this is the single place
/// the no-write flag is attached.
fn args<'a>(rest: &[&'a str]) -> Vec<&'a str> {
    let mut v = Vec::with_capacity(rest.len() + 1);
    v.extend_from_slice(rest);
    v.push(IGNORE_WORKING_COPY);
    v
}

/// Runs jj in `cwd` and returns its stdout, or None if it could not run or exited non-zero.
fn run(cwd: &Path, rest: &[&str]) -> Option<String> {
    let out = std::process::Command::new("jj")
        .current_dir(cwd)
        .args(args(rest))
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The workspace root: the nearest ancestor holding `.jj`.
///
/// Pure filesystem lookup — **jj is never launched for a directory that has no `.jj`**, which is
/// what keeps konoma's cost at zero in the repositories that don't use it.
pub fn workspace_root(root: &Path) -> Option<PathBuf> {
    let mut cur = Some(root);
    while let Some(dir) = cur {
        if dir.join(".jj").exists() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Whether this machine's jj can answer what konoma asks.
///
/// Probed once, by **running the query konoma depends on** rather than by comparing version
/// numbers: jj is pre-1.0 and moves monthly, so "does this actually work here" is both stricter and
/// more honest than "is the version at least X". If the probe fails, jj support switches itself off
/// and konoma falls back to git, exactly as it does when the git binary is missing.
pub fn available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("jj")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// What konoma needs to know about the working-copy commit and its parent.
#[derive(Debug, Clone)]
pub struct Meta {
    /// Shortened change ID, to the same 8 characters jj's own output uses — the identity jj users
    /// refer to. It survives rewrites, so it is what the chip and the graph show.
    pub change: String,
    /// First line of the description, empty when none was written yet (the common state in jj).
    pub summary: String,
    /// When the working copy was last snapshotted, in epoch seconds. Files newer than this are the
    /// ones jj has not seen yet.
    pub snapshot_epoch: i64,
}

/// The template is one line of tab-separated fields rather than jj's `json()`, so konoma needs no
/// JSON parser and works with jj builds that predate `json()`.
const META_TEMPLATE: &str = concat!(
    r#"change_id.shortest(8) ++ "\t" ++ committer.timestamp().format("%s")"#,
    r#" ++ "\t" ++ description.first_line() ++ "\n""#,
);

pub fn meta(ws: &Path) -> Option<Meta> {
    let line = run(ws, &["log", "-r", "@", "--no-graph", "-T", META_TEMPLATE])?;
    let line = line.lines().next()?;
    let mut f = line.split('\t');
    let change = f.next()?.to_string();
    let snapshot_epoch = f.next()?.trim().parse::<i64>().ok()?;
    let summary = f.next().unwrap_or("").to_string();
    if change.is_empty() {
        return None;
    }
    Some(Meta {
        change,
        summary,
        snapshot_epoch,
    })
}

/// Label for the tree's chip: the working-copy commit, the way jj names it.
///
/// git's "current branch" has no counterpart here — jj tracks no branch, and in a colocated
/// repository git itself is left detached, so the honest answer is the change ID plus what the
/// commit is about.
pub fn branch(root: &Path) -> Option<String> {
    let ws = workspace_root(root)?;
    let m = meta(&ws)?;
    Some(if m.summary.is_empty() {
        format!("@ {}", m.change)
    } else {
        format!("@ {} {}", m.change, m.summary)
    })
}

/// Paths jj already knows differ, as of the last snapshot. Stale by construction — the walk below
/// is what makes the answer current.
fn known_changes(ws: &Path) -> HashMap<String, FileStatus> {
    let mut map = HashMap::new();
    let Some(out) = run(ws, &["diff", "--summary"]) else {
        return map;
    };
    for line in out.lines() {
        let Some((mark, path)) = line.split_once(' ') else {
            continue;
        };
        let st = match mark {
            "A" => FileStatus::Added,
            "D" => FileStatus::Deleted,
            "C" => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        map.insert(path.to_string(), st);
    }
    map
}

/// The files the parent commit tracks. Anything on disk that is not here is an addition; anything
/// here that is not on disk is a deletion.
fn tracked(ws: &Path) -> HashSet<String> {
    run(ws, &["file", "list", "-r", "@-"])
        .map(|o| o.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

/// Whether `rel` really differs from the parent commit, by reading both sides.
fn differs_from_parent(ws: &Path, rel: &str, disk: &Path) -> bool {
    let Ok(now) = std::fs::read(disk) else {
        return true; // unreadable: report it rather than hide it
    };
    match parent_bytes(ws, rel) {
        Some(before) => before != now,
        None => true,
    }
}

/// One scan of the working copy: every file that differs from `@-`, as an absolute path.
///
/// **This is the only place the comparison is made** — both the tree's markers and the changed-file
/// list are derived from it, so they cannot drift apart.
///
/// Freshness comes from the walk, not from jj: `jj diff` answers from the last snapshot, so a file
/// written since then would be invisible to it. Every file newer than that snapshot is therefore a
/// candidate and gets checked against the parent's content — normally a handful, since any jj
/// command the user runs refreshes the snapshot.
fn scan(ws: &Path) -> Vec<(PathBuf, FileStatus)> {
    let mut out = Vec::new();
    let Some(m) = meta(ws) else {
        return out; // jj could not answer: stay quiet rather than guess
    };
    let known = known_changes(ws);
    let tracked = tracked(ws);
    let mut seen: HashSet<String> = HashSet::new();
    let mut confirmed = 0usize;

    for entry in walk(ws) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(rel) = entry.path().strip_prefix(ws).ok().and_then(|p| p.to_str()) else {
            continue;
        };
        let rel = rel.to_string();
        seen.insert(rel.clone());
        let st = if let Some(&st) = known.get(&rel) {
            st
        } else if !newer_than(entry.path(), m.snapshot_epoch) {
            continue; // jj has seen this file and reported nothing about it
        } else if !tracked.contains(&rel) {
            FileStatus::Added
        } else if confirmed < CONFIRM_CAP {
            confirmed += 1;
            if differs_from_parent(ws, &rel, entry.path()) {
                FileStatus::Modified
            } else {
                continue;
            }
        } else {
            FileStatus::Modified
        };
        out.push((crate::git::normalize_status_path(entry.into_path()), st));
    }

    for rel in tracked.difference(&seen) {
        out.push((ws.join(rel), FileStatus::Deleted));
    }
    out
}

/// Status of everything that differs from `@-`, as absolute paths, with directories rolled up the
/// same way the git backend does it.
pub fn statuses(root: &Path) -> HashMap<PathBuf, FileStatus> {
    let mut map = HashMap::new();
    let Some(ws) = workspace_root(root) else {
        return map;
    };
    for (abs, st) in scan(&ws) {
        crate::git::rollup(&mut map, &ws, &abs, st);
    }
    map
}

/// The changed files, one entry per file, sorted by path — what the changed-file list (`C`) and the
/// jumps between changes (`n`/`N`) walk.
///
/// `staged` is always false: jj has no index, so there is no such distinction to report. The hub
/// hides the staging keys for the same reason.
pub fn changed_files(root: &Path) -> Vec<crate::git::ChangeEntry> {
    let Some(ws) = workspace_root(root) else {
        return Vec::new();
    };
    let mut out: Vec<crate::git::ChangeEntry> = scan(&ws)
        .into_iter()
        .map(|(path, status)| crate::git::ChangeEntry {
            path,
            status,
            staged: false,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Working-copy diff of one file: the parent commit (`@-`) against what is on disk right now.
///
/// That base is what `jj diff` itself shows, and reading the current bytes off disk rather than
/// asking jj for them is what keeps the answer honest — jj would report its last snapshot, which
/// can be older than the file.
///
/// A file the parent does not have reads as all-added, matching how the git backend treats an
/// untracked file. Returns an empty diff when the file is outside the workspace or jj cannot
/// answer, the same "quietly render blank" contract as everywhere else here.
pub fn file_diff(root: &Path, file: &Path) -> Vec<crate::git::DiffLine> {
    let mut out = Vec::new();
    let Some(ws) = workspace_root(root) else {
        return out;
    };
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let ws_abs = ws.canonicalize().unwrap_or_else(|_| ws.clone());
    let Ok(rel) = abs.strip_prefix(&ws_abs) else {
        return out; // outside this workspace
    };
    let Some(rel_str) = rel.to_str() else {
        return out;
    };
    let before = parent_bytes(&ws, rel_str);
    let after = std::fs::read(&abs).ok();
    crate::git::push_file_diff(&mut out, rel, before.as_deref(), after.as_deref(), false);
    out
}

/// The parent commit's bytes for one path, or None when the parent does not have it.
fn parent_bytes(ws: &Path, rel: &str) -> Option<Vec<u8>> {
    revision_bytes(ws, "@-", rel)
}

/// Paths the ignore rules exclude, as absolute paths, with a fully ignored directory collapsed to a
/// single entry rather than recursed into — the same shape the git backend returns, because the tree
/// reads it as "self or an ancestor is in this set".
///
/// jj has no `git status --ignored` to delegate to, so the set is derived: walk once with the ignore
/// rules applied to learn what survives them, then walk the directory tree and record whatever did
/// not. Descending stops at each ignored entry, so a `target/` or `node_modules/` is one entry and
/// is never entered.
pub fn ignored(root: &Path) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    let Some(ws) = workspace_root(root) else {
        return set;
    };
    let visible: HashSet<PathBuf> = walk(&ws).map(ignore::DirEntry::into_path).collect();
    let mut stack = vec![ws];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".jj" || name == ".git" {
                continue; // the VCS's own directory, which git does not report either
            }
            if !visible.contains(&path) {
                set.insert(path); // ignored: record it and do not look inside
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            }
        }
    }
    set
}

/// The workspace walk: everything jj would consider part of the working copy.
///
/// `require_git(false)` is what makes `.gitignore` apply in a repository created with
/// `jj git init --no-colocate`, which has no `.git` for the ignore crate to key off.
fn walk(ws: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(ws)
        .hidden(false) // jj tracks dotfiles like any other file
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".jj" && e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
}

/// Whether the file was written after the given snapshot.
fn newer_than(path: &Path, epoch: i64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(mtime) = meta.modified() else {
        return true;
    };
    match mtime.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64 >= epoch,
        Err(_) => true,
    }
}

// --- The hub: history, bookmarks and commit detail -----------------------------------------------
// jj names commits by change ID, so that is what every list shows; the commit ID stays available for
// the detail view, which is where it is needed to talk to git, GitHub or CI.

/// Fields shared by the history readers, tab-separated so no JSON parser is needed.
const DAG_TEMPLATE: &str = concat!(
    r#"commit_id ++ "\t" ++ parents.map(|p| p.commit_id()).join(" ") ++ "\t""#,
    r#" ++ change_id.shortest(8) ++ "\t" ++ description.first_line() ++ "\t""#,
    r#" ++ author.name() ++ "\t" ++ committer.timestamp().format("%Y-%m-%d") ++ "\t""#,
    r#" ++ committer.timestamp().format("%s") ++ "\t" ++ working_copies ++ "\t""#,
    r#" ++ bookmarks.join(",") ++ "\t" ++ if(conflict,"c","") ++ if(immutable,"i","")"#,
    r#" ++ if(current_working_copy,"w","") ++ "\n""#,
);

/// One row of `jj log`, before konoma decides how to draw it.
struct Row {
    commit: String,
    parents: Vec<String>,
    change: String,
    subject: String,
    author: String,
    date: String,
    epoch: i64,
    /// `default@` / `ws-second@` — which workspace has this commit checked out, if any.
    workspaces: String,
    bookmarks: String,
    conflict: bool,
    immutable: bool,
    /// Checked out by *this* workspace. Another workspace's checkout is an ordinary commit that
    /// happens to carry a label — jj marks only its own with `@`.
    here: bool,
}

/// Reads `revset` out of jj. `None` uses jj's own default, which is deliberately narrow: it hides
/// the commits jj rewrote on its way here, which is exactly the noise a full DAG walk would show.
fn rows(ws: &Path, revset: Option<&str>, max: usize) -> Vec<Row> {
    let mut call = vec!["log", "--no-graph", "-T", DAG_TEMPLATE];
    if let Some(r) = revset {
        call.push("-r");
        call.push(r);
    }
    let Some(out) = run(ws, &call) else {
        return Vec::new();
    };
    out.lines()
        .take(max)
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 10 {
                return None;
            }
            Some(Row {
                commit: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                change: f[2].to_string(),
                subject: f[3].to_string(),
                author: f[4].to_string(),
                date: f[5].to_string(),
                epoch: f[6].trim().parse().unwrap_or(0),
                workspaces: f[7].to_string(),
                bookmarks: f[8].to_string(),
                conflict: f[9].contains('c'),
                immutable: f[9].contains('i'),
                here: f[9].contains('w'),
            })
        })
        .collect()
}

/// What decorates a row in the graph: the workspaces that have it checked out and the bookmarks
/// pointing at it. `default@` matters as much as a bookmark name here — it is how a jj user reading
/// several workspaces at once tells them apart.
fn decorations(r: &Row) -> String {
    [r.workspaces.as_str(), r.bookmarks.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

fn node_kind(r: &Row) -> crate::git::NodeKind {
    use crate::git::NodeKind as K;
    if r.conflict {
        K::Conflict
    } else if r.here {
        K::WorkingCopy
    } else if r.immutable {
        K::Immutable
    } else if r.parents.len() >= 2 {
        K::Merge
    } else {
        K::Normal
    }
}

/// The commit list behind `l`.
pub fn log(root: &Path, max: usize) -> Vec<crate::git::CommitInfo> {
    let Some(ws) = workspace_root(root) else {
        return Vec::new();
    };
    rows(&ws, None, max)
        .into_iter()
        .map(|r| crate::git::CommitInfo {
            id: r.commit,
            // What a jj user reads and retypes is the change ID; it survives the rewrites that give
            // a commit a new hash.
            short: r.change,
            summary: r.subject,
            author: r.author,
            time_epoch: r.epoch,
        })
        .collect()
}

/// The commit graph.
///
/// The range is jj's own default rather than every reachable commit: jj rewrites commits as you
/// work, and all of those predecessors stay reachable through `refs/jj/keep/*`, so walking the
/// whole DAG buries the six commits that matter under dozens that no longer exist to the user.
/// Passing an explicit revset (`all()`) is what widens it again.
pub fn graph(root: &Path, revset: Option<&str>, max: usize) -> Vec<crate::git::GraphRow> {
    let Some(ws) = workspace_root(root) else {
        return Vec::new();
    };
    let commits: Vec<crate::git::DagCommit> = rows(&ws, revset, max)
        .into_iter()
        .map(|r| crate::git::DagCommit {
            kind: Some(node_kind(&r)),
            refs: decorations(&r),
            id: r.commit,
            short: r.change,
            subject: if r.immutable && r.parents.is_empty() && r.subject.is_empty() {
                "root()".to_string() // jj's own name for it; the alternative is a blank row
            } else {
                r.subject
            },
            author: if r.epoch == 0 {
                String::new()
            } else {
                r.author
            },
            date: if r.epoch == 0 { String::new() } else { r.date },
            parents: r.parents,
        })
        .collect();
    // No pseudo-row for uncommitted work: in jj the working copy *is* a commit, and it is already
    // in the list above.
    crate::git::lay_out_lanes(&commits, None, None, crate::vcs::VcsKind::Jj)
}

/// Bookmarks, in the shape the branch list already renders.
///
/// `is_current` is always false: a jj bookmark does not follow the working copy the way a git branch
/// follows HEAD, so there is no "the one you are on" to mark. Working on an unnamed commit is the
/// normal state, not a detached-HEAD accident.
pub fn bookmarks(root: &Path) -> Vec<crate::git::BranchInfo> {
    let Some(ws) = workspace_root(root) else {
        return Vec::new();
    };
    let Some(out) = run(&ws, &["bookmark", "list", "-T", r#"name ++ "\n""#]) else {
        return Vec::new();
    };
    let mut v: Vec<crate::git::BranchInfo> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|name| crate::git::BranchInfo {
            name: name.to_string(),
            is_current: false,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v.dedup_by(|a, b| a.name == b.name);
    v
}

/// The commit a bookmark points at.
pub fn bookmark_tip(root: &Path, name: &str) -> Option<String> {
    let ws = workspace_root(root)?;
    let out = run(&ws, &["log", "-r", name, "--no-graph", "-T", "commit_id"])?;
    let id = out.lines().next()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Detail for one revision. `id` may be a commit ID or a change ID — jj resolves both.
///
/// The description comes last and is read whole rather than flattened: it is the only field that
/// can hold newlines, so putting it at the end means nothing has to escape them.
pub fn commit_meta(root: &Path, id: &str) -> Option<crate::git::CommitMeta> {
    let ws = workspace_root(root)?;
    const T: &str = concat!(
        r#"commit_id ++ "\t" ++ change_id.shortest(8) ++ "\t" ++ author.name() ++ "\t""#,
        r#" ++ committer.timestamp().format("%Y-%m-%d %H:%M") ++ "\t" ++ description"#,
    );
    let out = run(&ws, &["log", "-r", id, "--no-graph", "-T", T])?;
    let mut f = out.splitn(5, '\t');
    let commit = f.next()?.to_string();
    let change = f.next()?.to_string();
    let author = f.next()?.to_string();
    let date = f.next()?.to_string();
    let message = f.next().unwrap_or("").trim_end().to_string();
    if commit.is_empty() {
        return None;
    }
    Some(crate::git::CommitMeta {
        id: commit,
        // The change ID leads, because that is the name this commit keeps across rewrites.
        short: change,
        author,
        date,
        message,
    })
}

/// Everything one revision changed, file by file, with a header per file — the same shape the git
/// backend produces, so the detail view renders it unchanged.
pub fn commit_diff(root: &Path, id: &str) -> Vec<crate::git::DiffLine> {
    let mut out = Vec::new();
    let Some(ws) = workspace_root(root) else {
        return out;
    };
    let Some(summary) = run(&ws, &["diff", "--summary", "-r", id]) else {
        return out;
    };
    let parent = format!("{id}-");
    for line in summary.lines() {
        let Some((_, rel)) = line.split_once(' ') else {
            continue;
        };
        let before = revision_bytes(&ws, &parent, rel);
        let after = revision_bytes(&ws, id, rel);
        crate::git::push_file_diff(
            &mut out,
            Path::new(rel),
            before.as_deref(),
            after.as_deref(),
            true,
        );
    }
    out
}

/// One path's bytes at one revision, or None when that revision does not have it.
fn revision_bytes(ws: &Path, rev: &str, rel: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("jj")
        .current_dir(ws)
        .args(args(&["file", "show", "-r", rev, rel]))
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one invariant that keeps konoma from writing to a jj repository. If a call is ever built
    /// without going through `args`, this test is the thing that should have caught it.
    #[test]
    fn every_invocation_refuses_to_snapshot() {
        for call in [
            vec!["log", "-r", "@"],
            vec!["diff", "--summary"],
            vec!["file", "list", "-r", "@-"],
            vec!["file", "show", "-r", "@-", "a.txt"],
        ] {
            assert!(
                args(&call).contains(&IGNORE_WORKING_COPY),
                "a jj call would have snapshotted the working copy: {call:?}"
            );
        }
    }

    /// The flag goes last, so it can never be swallowed as the value of a preceding option.
    #[test]
    fn the_flag_is_appended_not_inserted() {
        let a = args(&["log", "-r", "@"]);
        assert_eq!(a.first(), Some(&"log"));
        assert_eq!(a.last(), Some(&IGNORE_WORKING_COPY));
    }

    /// A directory with no `.jj` anywhere above it must not make konoma launch jj at all.
    #[test]
    fn no_marker_means_no_workspace() {
        assert!(workspace_root(Path::new("/")).is_none());
    }
}
