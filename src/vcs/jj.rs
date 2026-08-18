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
    let out = std::process::Command::new("jj")
        .current_dir(ws)
        .args(args(&["file", "show", "-r", "@-", rel]))
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => o.stdout != now,
        _ => true,
    }
}

/// Status of everything that differs from `@-`, as absolute paths, with directories rolled up the
/// same way the git backend does it.
///
/// Freshness comes from the walk, not from jj: `jj diff` answers from the last snapshot, so a file
/// written since then would be invisible. Every file newer than that snapshot is therefore treated
/// as a candidate and checked against the parent's content — normally a handful, since the snapshot
/// is refreshed by any jj command the user runs.
pub fn statuses(root: &Path) -> HashMap<PathBuf, FileStatus> {
    let mut map = HashMap::new();
    let Some(ws) = workspace_root(root) else {
        return map;
    };
    let Some(m) = meta(&ws) else {
        return map; // jj could not answer: stay quiet rather than guess
    };
    let known = known_changes(&ws);
    let tracked = tracked(&ws);
    let mut seen: HashSet<String> = HashSet::new();
    let mut confirmed = 0usize;

    for entry in walk(&ws) {
        let Ok(rel) = entry.path().strip_prefix(&ws) else {
            continue;
        };
        let Some(rel) = rel.to_str() else {
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        seen.insert(rel.to_string());
        let st = if let Some(&st) = known.get(rel) {
            st
        } else if !newer_than(entry.path(), m.snapshot_epoch) {
            continue; // jj has seen this file and reported nothing about it
        } else if !tracked.contains(rel) {
            FileStatus::Added
        } else if confirmed < CONFIRM_CAP {
            confirmed += 1;
            if differs_from_parent(&ws, rel, entry.path()) {
                FileStatus::Modified
            } else {
                continue;
            }
        } else {
            FileStatus::Modified
        };
        crate::git::rollup(
            &mut map,
            &ws,
            &crate::git::normalize_status_path(entry.into_path()),
            st,
        );
    }

    for rel in tracked.difference(&seen) {
        crate::git::rollup(&mut map, &ws, &ws.join(rel), FileStatus::Deleted);
    }
    map
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
