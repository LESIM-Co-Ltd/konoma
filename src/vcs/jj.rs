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

/// **The one command konoma runs without `--ignore-working-copy`**, and only when the user asks for
/// it explicitly.
///
/// It makes jj take a snapshot: jj compares the working directory against `@` and, if they differ,
/// rewrites `@` to match. That is a write — a new commit and an entry in the operation log — which
/// is why nothing else here does it. It exists as a way out of the one thing konoma's own tracking
/// cannot see: a file whose contents changed while its modification time did not.
///
/// Runs synchronously: the user just confirmed a dialog, and jj does nothing when there is nothing
/// to take (measured: no new commit, no operation, no bytes). It only costs anything when there is
/// something to snapshot, which is exactly when the user asked for it.
pub fn snapshot(root: &Path) -> bool {
    let Some(ws) = workspace_root(root) else {
        return false;
    };
    std::process::Command::new("jj")
        .current_dir(&ws)
        .args(["status"]) // deliberately not through `args`: see above
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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
    parse_meta_line(line)
}

/// Parses one [`META_TEMPLATE`] line. `splitn(3, ...)` rather than a plain `split` so a tab
/// embedded in the trailing `summary` field — the only free-text field here — is kept whole
/// instead of silently truncating `summary` at the first tab it happens to contain.
fn parse_meta_line(line: &str) -> Option<Meta> {
    let mut f = line.splitn(3, '\t');
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

/// One line of a jj diff read, parsed. `path` is always where the file ends up — the plain path
/// for an add/modify/delete, the destination for a rename or copy. `from` is only set when a
/// rename or copy makes the origin path different from `path`; it is None whenever the two sides
/// are the same file at the same path.
struct SummaryEntry {
    status: FileStatus,
    path: String,
    from: Option<String>,
}

/// Parses `jj diff --summary` into one [`SummaryEntry`] per line.
///
/// **This is the fallback path**, used only when [`diff_template_available`] says this jj cannot
/// parse [`DIFF_TEMPLATE`] — see [`diff_entries`], which is what every caller in this module
/// actually goes through. It is kept, and kept tested, because it is what genuinely runs on an
/// older jj, not because it is retained for history's sake.
///
/// Every line is `<mark> <path>`, except a rename (`R`), which compresses the common prefix and
/// suffix around the part that changed: `PREFIX{FROM => TO}SUFFIX` (see [`split_rename`]). Only
/// `R` lines are treated that way — a plain `A`/`M`/`D`/`C` path is used verbatim, so a filename
/// that happens to contain literal `{... => ...}` text is never mistaken for a rename shape.
/// That compression is genuinely ambiguous for two shapes of filename at once — one containing a
/// literal `{`/`}`, one containing the literal `" => "` separator itself — which [`split_rename`]
/// cannot resolve correctly for both; [`diff_entries`] prefers the template precisely so this
/// parse, and its ambiguity, is never reached on a jj new enough to avoid it.
fn parse_diff_summary(out: &str) -> Vec<SummaryEntry> {
    let mut v = Vec::new();
    for line in out.lines() {
        let Some((mark, rest)) = line.split_once(' ') else {
            continue;
        };
        if mark == "R" {
            // A rename jj marked but this module could not parse is dropped rather than guessed
            // at — better to under-report than to invent a path that does not exist.
            if let Some((from, to)) = split_rename(rest) {
                v.push(SummaryEntry {
                    status: FileStatus::Renamed,
                    path: to,
                    from: Some(from),
                });
            }
            continue;
        }
        let status = match mark {
            "A" => FileStatus::Added,
            "D" => FileStatus::Deleted,
            "C" => FileStatus::Renamed, // copy: jj has no separate FileStatus for it
            _ => FileStatus::Modified,
        };
        v.push(SummaryEntry {
            status,
            path: rest.to_string(),
            from: None,
        });
    }
    v
}

/// Splits `PREFIX{FROM => TO}SUFFIX` — the compressed form jj (and git's own `--summary`) use for
/// a rename — into the origin and destination paths, `PREFIX+FROM+SUFFIX` and `PREFIX+TO+SUFFIX`.
/// `PREFIX`, `SUFFIX`, `FROM` and `TO` may each be empty; only the `{`, `=>` and `}` are
/// load-bearing. Returns None when `spec` has no such structure, e.g. an unrecognized shape.
fn split_rename(spec: &str) -> Option<(String, String)> {
    let open = spec.find('{')?;
    let close = open + spec[open..].find('}')?;
    let (from, to) = spec[open + 1..close].split_once(" => ")?;
    let prefix = &spec[..open];
    let suffix = &spec[close + 1..];
    Some((
        join_rename_side(prefix, from, suffix),
        join_rename_side(prefix, to, suffix),
    ))
}

/// Puts one side of a compressed rename back together.
///
/// Either side can be empty — moving `a/b/c.txt` up to `a/c.txt` prints `a/{b => }/c.txt` — and
/// then the separators around the empty part collide, so a plain concatenation would yield
/// `a//c.txt`: a path no walk of the working copy will ever produce, which is exactly how a
/// renamed file loses its marker. jj never emits a doubled separator otherwise, so collapsing them
/// (and any separator left leading the whole path) restores the real path without guessing.
fn join_rename_side(prefix: &str, part: &str, suffix: &str) -> String {
    let joined = format!("{prefix}{part}{suffix}");
    let collapsed = joined.replace("//", "/");
    collapsed.trim_start_matches('/').to_string()
}

/// The `-T` template [`diff_entries`] prefers over `--summary`: one line per changed entry,
/// printing the *actual* source and target path rather than compressing them into `--summary`'s
/// `PREFIX{FROM => TO}SUFFIX` shape. `self.` is required on every method — jj's diff-entry template
/// language binds nothing bare.
const DIFF_TEMPLATE: &str = concat!(
    r#"self.status_char() ++ "\t" ++ self.source().path() ++ "\t""#,
    r#" ++ self.target().path() ++ "\n""#,
);

/// Whether this jj understands the diff-entry template methods [`DIFF_TEMPLATE`] uses
/// (`self.status_char()`, `self.source()`, `self.target()`). Probed once, by running the query
/// itself rather than comparing version numbers — the same reasoning as [`available`]: pre-1.0 jj
/// moves too fast for a version compare to be trustworthy, and only actually running the template
/// proves the parser accepts it. Probing against `@` (which always exists, unlike a caller-supplied
/// revision) keeps the verdict a property of this jj binary, not of what a particular call happens
/// to be asking about.
///
/// `OnceLock` means at most one extra jj invocation is ever spent finding this out, and only for
/// the very first diff read in the process; every read after it already knows the answer and goes
/// straight to the template or straight to `--summary`.
fn diff_template_available(ws: &Path) -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| run(ws, &["diff", "-r", "@", "-T", DIFF_TEMPLATE]).is_some())
}

/// Reads one jj diff — the working-copy commit against its parent when `revset` is None, or a
/// specific revision's diff when `Some` — into [`SummaryEntry`] rows.
///
/// **Both [`known_changes`] and [`commit_diff`] read through here.** That is deliberate: they must
/// always agree on what counts as a rename, and routing both through one function is what makes
/// that automatic instead of something to keep in sync by hand as the two evolve separately.
///
/// Prefers the `-T` template ([`parse_diff_template`]): it prints the real source and target path
/// of every entry, so there is no compressed shape to reverse — which is what makes it correct
/// where `--summary`'s compression is not (see [`parse_diff_summary`]'s doc comment). Falls back to
/// `--summary` ([`parse_diff_summary`]) when [`diff_template_available`] says this jj cannot parse
/// the template.
fn diff_entries(ws: &Path, revset: Option<&str>) -> Vec<SummaryEntry> {
    let mut call = vec!["diff"];
    if let Some(r) = revset {
        call.push("-r");
        call.push(r);
    }
    if diff_template_available(ws) {
        call.push("-T");
        call.push(DIFF_TEMPLATE);
        return run(ws, &call)
            .map(|out| parse_diff_template(&out))
            .unwrap_or_default();
    }
    call.push("--summary");
    run(ws, &call)
        .map(|out| parse_diff_summary(&out))
        .unwrap_or_default()
}

/// Parses one line of [`DIFF_TEMPLATE`]'s output: `<status_char>\t<source path>\t<target path>`.
///
/// Unlike `--summary`, the source and target are printed in full rather than compressed, so there
/// is nothing to reverse-engineer: a rename or copy is exactly the case where the two differ, and
/// every other status (`A`/`M`/`D`) prints the same path on both sides.
///
/// `splitn(3, '\t')`, for the same reason [`parse_dag_line`] and [`parse_meta_line`] do it: a path
/// could itself contain a tab, and the trailing field (the target) is the only one that needs to
/// survive that here, so letting it absorb everything after the second tab is what keeps such a
/// path whole instead of splitting it into more fields than the line actually carries. A line with
/// fewer than three fields is dropped — there is nothing meaningful to report from it.
fn parse_diff_template(out: &str) -> Vec<SummaryEntry> {
    let mut v = Vec::new();
    for line in out.lines() {
        let mut f = line.splitn(3, '\t');
        let Some(mark) = f.next() else { continue };
        let Some(source) = f.next() else { continue };
        let Some(target) = f.next() else { continue };
        let status = match mark {
            "A" => FileStatus::Added,
            "D" => FileStatus::Deleted,
            "C" | "R" => FileStatus::Renamed, // copy: jj has no separate FileStatus for it
            _ => FileStatus::Modified,
        };
        let from = (source != target).then(|| source.to_string());
        v.push(SummaryEntry {
            status,
            path: target.to_string(),
            from,
        });
    }
    v
}

/// Paths jj already knows differ, as of the last snapshot. Stale by construction — the walk below
/// is what makes the answer current.
fn known_changes(ws: &Path) -> HashMap<String, FileStatus> {
    diff_entries(ws, None)
        .into_iter()
        .map(|e| (e.path, e.status))
        .collect()
}

/// The files the parent commit tracks. Anything on disk that is not here is an addition; anything
/// here that is not on disk is a deletion.
fn tracked(ws: &Path) -> HashSet<String> {
    run(ws, &["file", "list", "-r", "@-"])
        .map(|o| parse_file_list(&o))
        .unwrap_or_default()
}

/// Parses `jj file list`'s output: one repo-relative path per line, symlinks included — jj tracks
/// a symlink the same way it tracks a file.
fn parse_file_list(out: &str) -> HashSet<String> {
    out.lines().map(|l| l.to_string()).collect()
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
        let Some(ft) = entry.file_type() else {
            continue; // e.g. a stat that failed mid-walk — nothing meaningful to report
        };
        let is_symlink = ft.is_symlink();
        if !ft.is_file() && !is_symlink {
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
            // Untracked as of the parent commit, and jj has not reported anything about it yet
            // either (it is not in `known`) — this must be a fresh addition, symlink or not. This
            // check has to come before the `is_symlink` branch below: that branch's fallback
            // reasoning only holds for a symlink jj already tracks, so checking it first would
            // read every never-tracked symlink as Modified instead of Added.
            FileStatus::Added
        } else if is_symlink {
            // A tracked symlink that changed since the last snapshot, but jj has not reported
            // anything about it yet (it is not in `known`). It cannot be confirmed the way a
            // regular file is below: `jj file show` returns zero bytes for a symlink rather than
            // its target path (see the module docs' note on the CLI's read-only surface), so
            // comparing that against `std::fs::read` of the link — which follows it to the *target
            // file's* content — would compare two unrelated things and could misreport either way.
            // Rather than guess, take the same optimistic stance the CONFIRM_CAP overflow path
            // takes: report Modified, erring towards showing a live change over hiding one.
            FileStatus::Modified
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
    // `symlink_metadata`, not `metadata`: for a symlink the thing that can have changed is the
    // link itself, and following it would read the target's timestamp instead — so retargeting a
    // link would go unnoticed while an untouched link pointing at a busy file would keep looking
    // changed. For a regular file the two are the same call.
    let Ok(meta) = std::fs::symlink_metadata(path) else {
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
///
/// `description.first_line()` is last, for the same reason `commit_meta`'s template puts the full
/// description last: it is the one free-text field, and jj puts no bound on what a description's
/// first line holds — including a literal tab, unlike every other field here, which comes from a
/// closed vocabulary (an ID, a digit string, a single-char flag set). It used to sit in the
/// middle, so a tab embedded in it grew the line to 11 tab-separated fields; `rows()`'s guard only
/// rejects too *few* fields, so the extra one passed through and shifted author, date, epoch,
/// workspaces, bookmarks and the flags one column to their left. The resulting `epoch` parse
/// failed (`unwrap_or(0)`), and 0 is the sentinel `graph()` uses for the root commit, so the row
/// lost its author and date; the flags column, now reading what used to be `epoch`, no longer
/// contained `w`, so the working copy stopped drawing its `@`. Putting the field last and parsing
/// with `splitn` (see [`DAG_FIELDS`]) lets it absorb any tab inside it instead.
const DAG_TEMPLATE: &str = concat!(
    r#"commit_id ++ "\t" ++ parents.map(|p| p.commit_id()).join(" ") ++ "\t""#,
    r#" ++ change_id.shortest(8) ++ "\t" ++ author.name() ++ "\t""#,
    r#" ++ committer.timestamp().format("%Y-%m-%d") ++ "\t""#,
    r#" ++ committer.timestamp().format("%s") ++ "\t" ++ working_copies ++ "\t""#,
    r#" ++ bookmarks.join(",") ++ "\t" ++ if(conflict,"c","") ++ if(immutable,"i","")"#,
    r#" ++ if(current_working_copy,"w","") ++ "\t" ++ description.first_line() ++ "\n""#,
);

/// How many tab-separated fields [`DAG_TEMPLATE`] has. `rows()` uses this both to reject a
/// malformed line and, via `splitn`, to let the trailing subject field absorb a tab of its own.
const DAG_FIELDS: usize = 10;

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
    out.lines().take(max).filter_map(parse_dag_line).collect()
}

/// Parses one [`DAG_TEMPLATE`] line into a [`Row`]. `splitn(DAG_FIELDS, ...)` rather than a plain
/// `split` is what keeps a tab embedded in the trailing subject field from being read as an
/// eleventh field and shifting every fixed-width field before it — see [`DAG_TEMPLATE`]'s docs.
fn parse_dag_line(line: &str) -> Option<Row> {
    let f: Vec<&str> = line.splitn(DAG_FIELDS, '\t').collect();
    if f.len() < DAG_FIELDS {
        return None;
    }
    Some(Row {
        commit: f[0].to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        change: f[2].to_string(),
        author: f[3].to_string(),
        date: f[4].to_string(),
        epoch: f[5].trim().parse().unwrap_or(0),
        workspaces: f[6].to_string(),
        bookmarks: f[7].to_string(),
        conflict: f[8].contains('c'),
        immutable: f[8].contains('i'),
        here: f[8].contains('w'),
        subject: f[9].to_string(),
    })
}

/// What decorates a row in the graph: the workspaces that have it checked out and the bookmarks
/// pointing at it. `default@` matters as much as a bookmark name here — it is how a jj user reading
/// several workspaces at once tells them apart.
fn decorations(r: &Row) -> String {
    [
        r.workspaces.as_str(),
        r.bookmarks.as_str(),
        if r.conflict { "conflict" } else { "" },
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(", ")
}

fn node_kind(r: &Row) -> crate::git::NodeKind {
    use crate::git::NodeKind as K;
    // Where you are outranks what is wrong with it — jj keeps drawing `@` for a conflicted working
    // copy and says "(conflict)" alongside, because a conflict there is a state to work in, not a
    // reason to stop. `decorations` carries the word.
    if r.here {
        K::WorkingCopy
    } else if r.conflict {
        K::Conflict
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
    parse_bookmark_list(&out)
}

/// Parses `jj bookmark list`'s output: one name per line, sorted and deduplicated. jj can report
/// the same bookmark more than once (e.g. local plus each remote it tracks); this collapses that
/// to one row per name, which is all the branch list has room to show.
fn parse_bookmark_list(out: &str) -> Vec<crate::git::BranchInfo> {
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
    parse_commit_meta(&out)
}

/// Parses one `commit_meta` template's output. Read as a single record rather than a line at a
/// time: `description` is the one field allowed to contain newlines, and taking only the first
/// line of `out` would truncate a multi-line commit message at its first line break.
fn parse_commit_meta(out: &str) -> Option<crate::git::CommitMeta> {
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
    let parent = format!("{id}-");
    for entry in diff_entries(&ws, Some(id)) {
        // A rename's "before" content lives at the origin path, not the destination — the
        // destination is new in the parent's tree and reading it there always answers None,
        // which used to make every renamed file's diff render as all-added. `from` is set only
        // for a rename; every other status reads the same path on both sides, unchanged.
        let before_rel = entry.from.as_deref().unwrap_or(&entry.path);
        let before = revision_bytes(&ws, &parent, before_rel);
        let after = revision_bytes(&ws, id, &entry.path);
        crate::git::push_file_diff(
            &mut out,
            Path::new(&entry.path),
            before.as_deref(),
            after.as_deref(),
            true,
        );
    }
    out
}

/// The `file:"<escaped>"` fileset literal `jj file show -r <rev> <fileset>` takes, built so a
/// path beginning with `-` can never be mistaken for an option the way a bare positional argument
/// would be (`jj file show -r <rev> -dash.txt` fails with "unexpected argument '-d'": jj's own
/// argument parser, not jj itself, rejects it before the fileset is ever evaluated). This is the
/// jj-side counterpart of `git.rs`'s `blob_spec`, which solves the same problem for git by always
/// prefixing the revision; jj's `file show` takes a bare fileset positional with no such prefix
/// trick available, so the escape has to live inside a fileset literal instead.
///
/// Order matters: backslashes are escaped first, then quotes — escaping the quote first would
/// double-escape the backslash just introduced for it.
fn fileset_literal(rel: &str) -> String {
    let escaped = rel.replace('\\', "\\\\").replace('"', "\\\"");
    format!("file:\"{escaped}\"")
}

/// One path's bytes at one revision, or None when that revision does not have it.
fn revision_bytes(ws: &Path, rev: &str, rel: &str) -> Option<Vec<u8>> {
    let literal = fileset_literal(rel);
    let out = std::process::Command::new("jj")
        .current_dir(ws)
        .args(args(&["file", "show", "-r", rev, &literal]))
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

    /// Every jj invocation in this module goes through `args` — which appends the no-write flag —
    /// except two that cannot write: the version probe, and the snapshot the user explicitly asks
    /// for. Checked against the source rather than trusted: the promise is only worth anything if a
    /// call that forgets the flag fails here.
    #[test]
    fn only_the_named_exceptions_run_jj_without_the_flag() {
        let src = include_str!("jj.rs");
        // Scan the module's code, not this test's own text.
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let mut unflagged = Vec::new();
        for (i, _) in code.match_indices("Command::new(\"jj\")") {
            let tail = &code[i..(i + 400).min(code.len())];
            if tail.contains(".args(args(") {
                continue;
            }
            let head = &code[..i];
            let name = head
                .rfind("pub fn ")
                .map(|j| head[j + "pub fn ".len()..].split('(').next().unwrap_or("?"))
                .unwrap_or("?");
            unflagged.push(name.to_string());
        }
        unflagged.sort();
        assert_eq!(
            unflagged,
            vec!["available", "snapshot"],
            "a jj call would snapshot the working copy without being asked"
        );
    }

    /// A directory with no `.jj` anywhere above it must not make konoma launch jj at all.
    #[test]
    fn no_marker_means_no_workspace() {
        assert!(workspace_root(Path::new("/")).is_none());
    }

    /// `Command::new(` appears exactly this many times outside the test module — one for each of
    /// `run`, `snapshot`, `available` and `revision_bytes`, the only places that spawn jj.
    ///
    /// `only_the_named_exceptions_run_jj_without_the_flag` above only catches a call spelled
    /// literally `Command::new("jj")`; a call written as `let prog = "jj"; Command::new(prog)`
    /// would slip past its string search without ever being noticed. Pinning the raw count of
    /// `Command::new(` occurrences catches that too, because *any* new call — however it spells
    /// the program name — changes the count.
    #[test]
    fn command_new_count_is_pinned() {
        let src = include_str!("jj.rs");
        // Scan the module's code, not this test's own text (which also contains the literal
        // string being searched for, inside `only_the_named_exceptions_run_jj_without_the_flag`).
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let count = code.matches("Command::new(").count();
        assert_eq!(
            count, 4,
            "the number of `Command::new(` calls in src/vcs/jj.rs changed — if you added a new \
             jj invocation, route it through args() so it carries --ignore-working-copy (unless \
             it deliberately does not write, like `snapshot`), then update this expected count \
             to match"
        );
    }

    /// `fileset_literal` always opens with `file:"` — the load-bearing property `revision_bytes`
    /// leans on: whatever `rel` is, the argument handed to jj is never the bare path, so a leading
    /// `-` can never reach jj's own argument parser.
    #[test]
    fn fileset_literal_never_hands_jj_a_bare_path() {
        for rel in [
            "-dash.txt",
            "has space.txt",
            "quote\".txt",
            "back\\slash.txt",
            "plain.txt",
        ] {
            let lit = fileset_literal(rel);
            assert!(
                lit.starts_with("file:\""),
                "a raw path must never reach jj's argument parser unescaped: {lit:?}"
            );
        }
    }

    /// The exact escaping, pinned so a future edit cannot silently reorder the two replacements
    /// (escaping the quote before the backslash would double-escape the backslash the quote
    /// escape just introduced).
    #[test]
    fn fileset_literal_escapes_quotes_and_backslashes() {
        assert_eq!(fileset_literal("-dash.txt"), "file:\"-dash.txt\"");
        assert_eq!(fileset_literal("has space.txt"), "file:\"has space.txt\"");
        assert_eq!(fileset_literal("quote\".txt"), "file:\"quote\\\".txt\"");
        assert_eq!(
            fileset_literal("back\\slash.txt"),
            "file:\"back\\\\slash.txt\""
        );
    }

    #[test]
    fn diff_summary_reads_plain_marks() {
        let entries = parse_diff_summary("A added.txt\nM modified.txt\nD deleted.txt\n");
        let got: Vec<(FileStatus, &str, Option<&str>)> = entries
            .iter()
            .map(|e| (e.status, e.path.as_str(), e.from.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (FileStatus::Added, "added.txt", None),
                (FileStatus::Modified, "modified.txt", None),
                (FileStatus::Deleted, "deleted.txt", None),
            ]
        );
    }

    #[test]
    fn diff_summary_keeps_a_space_in_the_path() {
        let entries = parse_diff_summary("M has space.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "has space.txt");
    }

    #[test]
    fn diff_summary_parses_every_compressed_rename_shape() {
        let entries = parse_diff_summary(
            "R {orig.txt => renamed.txt}\n\
             R {src => lib}/foo.txt\n\
             R {lib/foo.txt => foo.txt}\n\
             R {foo.txt => lib/foo.txt}\n",
        );
        let got: Vec<(&str, &str)> = entries
            .iter()
            .map(|e| (e.from.as_deref().unwrap_or(""), e.path.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("orig.txt", "renamed.txt"),
                ("src/foo.txt", "lib/foo.txt"),
                ("lib/foo.txt", "foo.txt"),
                ("foo.txt", "lib/foo.txt"),
            ]
        );
        assert!(entries.iter().all(|e| e.status == FileStatus::Renamed));
    }

    /// One side of the compressed rename is empty when a directory component is dropped, and the
    /// separators around it then collide. This is jj's real output for moving `a/b/c.txt` up to
    /// `a/c.txt`, captured from jj 0.44.0 — a naive concatenation reads the destination as
    /// `a//c.txt`, which matches nothing the working-copy walk produces, so the renamed file goes
    /// unmarked.
    #[test]
    fn diff_summary_rejoins_a_rename_whose_side_is_empty() {
        let entries = parse_diff_summary("R a/{b => }/c.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].from.as_deref(), Some("a/b/c.txt"));
        assert_eq!(
            entries[0].path, "a/c.txt",
            "the destination must be the path the tree walk sees, not a//c.txt"
        );
    }

    /// A plain (non-`R`) line whose filename itself contains `=>` must not be mistaken for a
    /// rename — only the `R` mark triggers [`split_rename`].
    #[test]
    fn diff_summary_does_not_misfire_on_a_filename_containing_arrow() {
        let entries = parse_diff_summary("M a=>b.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, FileStatus::Modified);
        assert_eq!(entries[0].path, "a=>b.txt");
        assert!(entries[0].from.is_none());
    }

    #[test]
    fn diff_summary_of_empty_input_is_empty() {
        assert!(parse_diff_summary("").is_empty());
    }

    #[test]
    fn diff_summary_tolerates_a_trailing_newline() {
        let entries = parse_diff_summary("A added.txt\n\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "added.txt");
    }

    /// The `-T` template's own rename shape: full paths, not `--summary`'s compressed
    /// `PREFIX{FROM => TO}SUFFIX`, so a filename containing a literal `{`/`}` is read correctly
    /// where `split_rename` would mistake the file's own braces for the compression markers.
    #[test]
    fn diff_template_reads_a_rename_whose_name_contains_curly_braces() {
        let entries = parse_diff_template("R\tweird{a}.txt\tweird{b}.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, FileStatus::Renamed);
        assert_eq!(entries[0].from.as_deref(), Some("weird{a}.txt"));
        assert_eq!(entries[0].path, "weird{b}.txt");
    }

    /// The other shape `--summary`'s compression cannot resolve: a filename containing the literal
    /// `" => "` separator itself. Reading full paths means there is nothing to reverse, so this
    /// never risks synthesizing a path (like `"src.txt => plaindest.txt"`) that does not exist.
    #[test]
    fn diff_template_reads_a_rename_whose_name_contains_the_literal_arrow() {
        let entries = parse_diff_template("R\tarrow => src.txt\tplaindest.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, FileStatus::Renamed);
        assert_eq!(entries[0].from.as_deref(), Some("arrow => src.txt"));
        assert_eq!(entries[0].path, "plaindest.txt");
    }

    /// Add/modify/delete always print the same path on both sides — `from` must read None, not
    /// `Some` of the same string, since [`SummaryEntry::from`] means "the origin differs".
    #[test]
    fn diff_template_reads_add_modify_delete_with_no_from() {
        let entries = parse_diff_template(
            "A\tadded.txt\tadded.txt\n\
             M\tmodified.txt\tmodified.txt\n\
             D\tdeleted.txt\tdeleted.txt\n",
        );
        let got: Vec<(FileStatus, &str, Option<&str>)> = entries
            .iter()
            .map(|e| (e.status, e.path.as_str(), e.from.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (FileStatus::Added, "added.txt", None),
                (FileStatus::Modified, "modified.txt", None),
                (FileStatus::Deleted, "deleted.txt", None),
            ]
        );
    }

    #[test]
    fn diff_template_reads_a_copy() {
        let entries = parse_diff_template("C\tsrc.txt\tcopy.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, FileStatus::Renamed); // jj has no separate FileStatus for it
        assert_eq!(entries[0].from.as_deref(), Some("src.txt"));
        assert_eq!(entries[0].path, "copy.txt");
    }

    #[test]
    fn diff_template_drops_a_line_with_too_few_fields() {
        assert!(parse_diff_template("A\tonly-one-field\n").is_empty());
        assert!(parse_diff_template("just-one-field-no-tab\n").is_empty());
    }

    #[test]
    fn diff_template_of_empty_input_is_empty() {
        assert!(parse_diff_template("").is_empty());
    }

    #[test]
    fn diff_template_tolerates_a_trailing_newline() {
        let entries = parse_diff_template("A\tadded.txt\tadded.txt\n\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "added.txt");
    }

    /// The regression this parser exists to avoid: a path can contain a tab, and it is always the
    /// trailing field (the target) here, since `splitn(3, ...)` only splits on the first two tabs.
    #[test]
    fn diff_template_keeps_a_tab_embedded_in_the_target_path() {
        let entries = parse_diff_template("R\tplain.txt\tweird\tname.txt\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path, "weird\tname.txt",
            "the target must absorb everything after the second tab, unsplit"
        );
        assert_eq!(entries[0].from.as_deref(), Some("plain.txt"));
    }

    fn dag_line(subject: &str, flags: &str) -> String {
        format!("commit1\tparent1\tchange1\tauthor\t2026-08-19\t1755600000\t\t\t{flags}\t{subject}")
    }

    #[test]
    fn dag_line_parses_a_normal_row() {
        let row = parse_dag_line(&dag_line("subject", "w")).expect("a well-formed row parses");
        assert_eq!(row.commit, "commit1");
        assert_eq!(row.parents, vec!["parent1"]);
        assert_eq!(row.change, "change1");
        assert_eq!(row.author, "author");
        assert_eq!(row.date, "2026-08-19");
        assert_eq!(row.epoch, 1_755_600_000);
        assert_eq!(row.subject, "subject");
        assert!(row.here);
        assert!(!row.conflict);
        assert!(!row.immutable);
    }

    /// The regression for bug 4: a tab embedded in the description must not shift every field
    /// that comes after it — there is nothing after it, since it is the trailing field.
    #[test]
    fn dag_line_keeps_a_tab_inside_the_description() {
        let line = dag_line("first\tsecond", "w");
        let row = parse_dag_line(&line).expect("a tab in the subject must not break the parse");
        assert_eq!(row.subject, "first\tsecond");
        assert_eq!(
            row.author, "author",
            "author must not shift: {}",
            row.author
        );
        assert_eq!(row.epoch, 1_755_600_000, "epoch must not shift");
        assert!(
            row.here,
            "the working-copy flag must still be read: {line:?}"
        );
    }

    #[test]
    fn dag_line_with_too_few_fields_is_dropped() {
        assert!(parse_dag_line("commit1\tparent1\tchange1").is_none());
    }

    /// A non-numeric epoch does not drop the row — it degrades to 0, the same sentinel a real
    /// root commit carries, matching the existing `unwrap_or(0)` fallback.
    #[test]
    fn dag_line_with_non_numeric_epoch_falls_back_to_zero() {
        let line = "commit1\tparent1\tchange1\tauthor\t2026-08-19\tnot-a-number\t\t\t\tsubject";
        let row = parse_dag_line(line).expect("a bad epoch must not drop the whole row");
        assert_eq!(row.epoch, 0);
    }

    fn meta_line(epoch: &str, summary: &str) -> String {
        format!("change1\t{epoch}\t{summary}")
    }

    #[test]
    fn meta_line_parses_normally() {
        let m = parse_meta_line(&meta_line("1755600000", "subject")).expect("well-formed");
        assert_eq!(m.change, "change1");
        assert_eq!(m.snapshot_epoch, 1_755_600_000);
        assert_eq!(m.summary, "subject");
    }

    /// The regression for bug 4's `META_TEMPLATE` half: a tab inside the trailing summary field
    /// must survive rather than being cut at the first tab `split` (not `splitn`) would stop at.
    #[test]
    fn meta_line_keeps_a_tab_inside_the_summary() {
        let m = parse_meta_line(&meta_line("1755600000", "first\tsecond")).expect("well-formed");
        assert_eq!(m.summary, "first\tsecond");
    }

    #[test]
    fn meta_line_with_empty_change_is_none() {
        assert!(parse_meta_line("\t1755600000\tsubject").is_none());
    }

    #[test]
    fn meta_line_with_non_numeric_epoch_is_none() {
        assert!(parse_meta_line("change1\tnot-a-number\tsubject").is_none());
    }

    #[test]
    fn bookmark_list_dedups_and_sorts() {
        let v = parse_bookmark_list("main\nfeature\n\nmain\n");
        let names: Vec<&str> = v.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["feature", "main"]);
        assert!(v.iter().all(|b| !b.is_current));
    }

    #[test]
    fn commit_meta_keeps_newlines_in_a_multi_line_description() {
        let out = "abc123\tchange1\tsomeone\t2026-08-19 12:00\tfirst line\nsecond line\nthird";
        let m = parse_commit_meta(out).expect("well-formed");
        assert_eq!(m.id, "abc123");
        assert_eq!(m.short, "change1");
        assert_eq!(m.author, "someone");
        assert_eq!(m.date, "2026-08-19 12:00");
        assert_eq!(m.message, "first line\nsecond line\nthird");
    }

    #[test]
    fn commit_meta_with_too_few_fields_is_none() {
        assert!(parse_commit_meta("abc123\tchange1").is_none());
    }

    /// Builds a throwaway jj repository with no colocated `.git`, so the assertions below can only
    /// pass through the jj backend. Returns None when this machine has no jj, which is the same
    /// answer konoma gives at runtime — the suite must stay green without it.
    ///
    /// Two commits, not one: `orig-for-rename.txt` is created in `genesis`, then renamed — with a
    /// small content edit, so jj's summary still calls it `R` rather than splitting it into a
    /// delete plus an add — to `renamed.txt` in `seed`. That is what lets `commit_diff` on `seed`
    /// exercise a real rename line.
    ///
    /// The 1.1s pause before `seed` is committed is not incidental: jj's timestamps are
    /// second-resolution, and `scan`'s `is_symlink` branch reads a symlink's marker *without* the
    /// content-compare fallback a regular file gets (see that branch's comment for why) — so for
    /// a symlink, unlike a regular file, whether it lands on the "already confirmed unchanged"
    /// path or the "assume changed" path is not merely an optimization, it decides the marker
    /// outright. The pause pushes every pre-seed fixture's mtime at least one full second behind
    /// the seed commit's own timestamp, so `newer_than` reads deterministically false for all of
    /// them regardless of how fast the surrounding test machinery happens to run — otherwise
    /// `symlink_that_is_only_in_the_snapshot_reports_no_marker` below would be flaky.
    fn scratch_repo(name: &str) -> Option<PathBuf> {
        if !available() {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("konoma_jj_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let jj = |args: &[&str]| {
            std::process::Command::new("jj")
                .current_dir(&dir)
                // A fresh HOME and an explicit identity: the machine running this may have no jj
                // config at all, and must not have this repository written into the one it does have.
                .env("HOME", &dir)
                .env("JJ_USER", "konoma test")
                .env("JJ_EMAIL", "test@example.invalid")
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !jj(&["git", "init", "--no-colocate", "."]) {
            return None;
        }
        std::fs::write(
            dir.join("orig-for-rename.txt"),
            b"line one\nline two\nline three\nline four\nline five\n",
        )
        .ok()?;
        if !jj(&["commit", "-m", "genesis"]) {
            return None;
        }
        std::fs::rename(dir.join("orig-for-rename.txt"), dir.join("renamed.txt")).ok()?;
        std::fs::write(
            dir.join("renamed.txt"),
            b"line one\nline two\nline three\nline four\nline five extended\n",
        )
        .ok()?;
        std::fs::write(dir.join("kept.txt"), b"one\n").ok()?;
        std::fs::write(dir.join("changed.txt"), b"before\n").ok()?;
        std::fs::write(dir.join("-dash-kept.txt"), b"dash kept\n").ok()?;
        std::fs::write(dir.join("-dash.txt"), b"dash before\n").ok()?;
        std::os::unix::fs::symlink("kept.txt", dir.join("link.txt")).ok()?;
        std::thread::sleep(std::time::Duration::from_millis(1100));
        if !jj(&["commit", "-m", "seed"]) {
            return None;
        }
        std::fs::write(dir.join("changed.txt"), b"after\n").ok()?;
        std::fs::write(dir.join("added.txt"), b"new\n").ok()?;
        std::fs::write(dir.join("-dash.txt"), b"dash after\n").ok()?;
        Some(dir)
    }

    /// A scratch repo whose seed commit renames two files whose names defeat `--summary`'s
    /// compressed `PREFIX{FROM => TO}SUFFIX` shape: one containing a literal `{`/`}`, one
    /// containing the literal `" => "` separator itself. `diff_entries` reads these through the
    /// `-T` template ([`DIFF_TEMPLATE`]) instead, which prints the source and target in full, so
    /// there is no compressed shape to reverse-engineer for either name.
    ///
    /// Same shape as `scratch_repo`'s own rename dance (mostly unchanged multi-line content, one
    /// line extended) for the same reason: jj's rename detection is similarity-based, and a file
    /// rewritten wholesale reads as a delete plus an add rather than a rename.
    fn scratch_repo_with_ambiguous_renames(name: &str) -> Option<PathBuf> {
        if !available() {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("konoma_jj_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let jj = |args: &[&str]| {
            std::process::Command::new("jj")
                .current_dir(&dir)
                .env("HOME", &dir)
                .env("JJ_USER", "konoma test")
                .env("JJ_EMAIL", "test@example.invalid")
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !jj(&["git", "init", "--no-colocate", "."]) {
            return None;
        }
        std::fs::write(
            dir.join("weird{a}.txt"),
            b"line one\nline two\nline three\nline four\nline five\n",
        )
        .ok()?;
        std::fs::write(
            dir.join("arrow => src.txt"),
            b"line one\nline two\nline three\nline four\nline five\n",
        )
        .ok()?;
        if !jj(&["commit", "-m", "genesis"]) {
            return None;
        }
        std::fs::rename(dir.join("weird{a}.txt"), dir.join("weird{b}.txt")).ok()?;
        std::fs::write(
            dir.join("weird{b}.txt"),
            b"line one\nline two\nline three\nline four\nline five extended\n",
        )
        .ok()?;
        std::fs::rename(dir.join("arrow => src.txt"), dir.join("plaindest.txt")).ok()?;
        std::fs::write(
            dir.join("plaindest.txt"),
            b"line one\nline two\nline three\nline four\nline five extended\n",
        )
        .ok()?;
        if !jj(&["commit", "-m", "seed"]) {
            return None;
        }
        Some(dir)
    }

    /// The backend has to see a change jj itself has not snapshotted yet — that gap is the whole
    /// reason konoma walks the tree instead of trusting `jj diff`.
    #[test]
    fn reports_changes_jj_has_not_snapshotted() {
        let Some(dir) = scratch_repo("statuses") else {
            return;
        };
        let st = statuses(&dir);
        assert_eq!(
            st.get(&dir.join("changed.txt")),
            Some(&FileStatus::Modified),
            "an edited file must be modified: {st:?}"
        );
        assert_eq!(
            st.get(&dir.join("added.txt")),
            Some(&FileStatus::Added),
            "a new file must be added, not untracked — jj has no untracked state: {st:?}"
        );
        assert!(
            !st.contains_key(&dir.join("kept.txt")),
            "an untouched file must carry no marker: {st:?}"
        );
        let files = changed_files(&dir);
        // changed.txt, -dash.txt and added.txt — the fixture also carries kept.txt,
        // -dash-kept.txt, the symlink and the renamed file, none of which were touched since the
        // seed commit, so none of them should contribute an entry (see the dedicated tests below).
        assert_eq!(files.len(), 3, "one entry per changed file: {files:?}");
        assert!(
            files.iter().all(|f| !f.staged),
            "jj has no index, so nothing can be staged: {files:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The diff is taken against the working-copy commit's parent and read off disk, so it shows the
    /// edit rather than the last snapshot.
    #[test]
    fn diffs_against_the_parent_commit() {
        let Some(dir) = scratch_repo("diff") else {
            return;
        };
        let lines = file_diff(&dir, &dir.join("changed.txt"));
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(
            text.contains(&"before"),
            "the parent's line is missing: {text:?}"
        );
        assert!(
            text.contains(&"after"),
            "the working copy's line is missing: {text:?}"
        );
        let added = file_diff(&dir, &dir.join("added.txt"));
        assert!(
            added.iter().all(|l| l.old_no.is_none()),
            "a file the parent does not have must read as all-added: {added:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The chip names the working-copy commit the way jj does, and never claims a branch.
    #[test]
    fn chip_names_the_working_copy_commit() {
        let Some(dir) = scratch_repo("chip") else {
            return;
        };
        let chip = branch(&dir).expect("a jj workspace always has a working-copy commit");
        assert!(
            chip.starts_with("@ "),
            "the chip must open with jj's own marker: {chip}"
        );
        assert!(
            !chip.contains("HEAD"),
            "jj tracks no branch and leaves git detached, so HEAD would be a lie: {chip}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for bug 1: a path beginning with `-` must be read like any other path, not
    /// dropped because jj's argument parser mistook it for a flag.
    #[test]
    fn dash_prefixed_path_is_never_read_as_a_flag() {
        let Some(dir) = scratch_repo("dash") else {
            return;
        };
        let st = statuses(&dir);
        assert_eq!(
            st.get(&dir.join("-dash.txt")),
            Some(&FileStatus::Modified),
            "an edited dash-prefixed file must be modified: {st:?}"
        );
        assert!(
            !st.contains_key(&dir.join("-dash-kept.txt")),
            "an untouched dash-prefixed file must carry no marker: {st:?}"
        );

        let lines = file_diff(&dir, &dir.join("-dash.txt"));
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(
            text.contains(&"dash before"),
            "the parent's line must be read — a bare `-dash.txt` argument fails jj's own \
             argument parser (exit 2, \"unexpected argument '-d'\"), which used to make \
             revision_bytes return None here: {text:?}"
        );
        assert!(
            text.contains(&"dash after"),
            "the working copy's line must be present too: {text:?}"
        );
        assert!(
            !lines.iter().all(|l| l.old_no.is_none()),
            "every line carrying no old-side number is the bug-1 symptom: with the parent read \
             silently failing, this file rendered as though it were entirely new: {lines:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for bug 3: a symlink jj still tracks must never be reported Deleted just
    /// because the working-copy walk used to skip every non-regular-file entry outright.
    #[test]
    fn symlink_that_is_only_in_the_snapshot_reports_no_marker() {
        let Some(dir) = scratch_repo("symlink") else {
            return;
        };
        let link = dir.join("link.txt");
        let st = statuses(&dir);
        assert!(
            !st.contains_key(&link),
            "a symlink untouched since the last snapshot must carry no marker — before the fix \
             it was excluded from the walk's `seen` set outright and so always fell out the other \
             end as Deleted, regardless of whether anything about it had changed: {st:?}"
        );
        assert!(
            changed_files(&dir).iter().all(|f| f.path != link),
            "and must not appear in the changed-file list either"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of bug 3: a symlink konoma *should* mark. Retargeting a link changes the
    /// link and nothing else, so its own timestamp is the only thing that moves — reading through
    /// it to the target's timestamp (which is what `metadata` does) would miss this entirely.
    #[test]
    fn retargeted_symlink_is_reported_as_changed() {
        let Some(dir) = scratch_repo("symlink_retarget") else {
            return;
        };
        let link = dir.join("link.txt");
        std::fs::remove_file(&link).expect("the fixture always creates this link");
        // Point it at a file that has *not* been touched since the seed commit, so the only thing
        // newer than the snapshot here is the link itself.
        std::os::unix::fs::symlink("-dash-kept.txt", &link).expect("retarget the link");

        let st = statuses(&dir);
        assert_eq!(
            st.get(&link),
            Some(&FileStatus::Modified),
            "a retargeted symlink must be reported: its own timestamp is the only one that moved, \
             so following the link to the target's timestamp hides the change entirely: {st:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for bug 2: a rename's diff must survive `commit_diff`, with the pre-image read
    /// from the origin path rather than (wrongly) from the destination.
    #[test]
    fn renamed_file_keeps_a_readable_pre_image_in_commit_diff() {
        let Some(dir) = scratch_repo("rename") else {
            return;
        };
        let seed = log(&dir, 10)
            .into_iter()
            .find(|c| c.summary == "seed")
            .expect("the fixture always creates a commit described \"seed\"");
        let lines = commit_diff(&dir, &seed.id);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(
            text.contains(&"renamed.txt"),
            "the renamed file must have an entry in the commit diff at all — with the summary \
             line parsed as `line.split_once(' ')`, the rename's key came out as the mangled \
             \"{{orig-for-rename.txt\", which read as absent on both sides and dropped the whole \
             file from the diff: {text:?}"
        );
        assert!(
            text.contains(&"line five"),
            "the origin's content must be readable from the move-from path: {text:?}"
        );
        assert!(
            text.contains(&"line five extended"),
            "the destination's content must be present too: {text:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for the `Added` vs `Modified` ordering fix: a symlink created after the seed
    /// commit — jj has never tracked it — must read as `Added`, the same as a brand-new regular
    /// file. Before the fix, `scan`'s `is_symlink` branch ran before the untracked check, so every
    /// never-tracked symlink read as `Modified` instead.
    #[test]
    fn symlink_created_after_the_last_snapshot_is_added_not_modified() {
        let Some(dir) = scratch_repo("symlink_added") else {
            return;
        };
        let link = dir.join("brand-new-link.txt");
        std::os::unix::fs::symlink("kept.txt", &link)
            .expect("create a fresh symlink jj has never tracked");

        let st = statuses(&dir);
        assert_eq!(
            st.get(&link),
            Some(&FileStatus::Added),
            "a symlink jj has never tracked must be Added, not Modified: {st:?}"
        );
        let files = changed_files(&dir);
        let entry = files
            .iter()
            .find(|f| f.path == link)
            .expect("the new symlink must appear in the changed-file list");
        assert_eq!(entry.status, FileStatus::Added);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for the rename-ambiguity fix: a literal `{`/`}` in a filename used to defeat
    /// `split_rename`'s `spec.find('{')` — the file's own braces were mistaken for the compression
    /// markers around `PREFIX{FROM => TO}SUFFIX`, so the inner `split_once(" => ")` failed and
    /// `parse_diff_summary` dropped the whole rename (see `split_rename`'s doc comment). Reading
    /// `-T DIFF_TEMPLATE` prints the source and target in full, so there is nothing to misparse.
    #[test]
    fn rename_with_curly_braces_in_the_name_survives_commit_diff() {
        let Some(dir) = scratch_repo_with_ambiguous_renames("curly") else {
            return;
        };
        let seed = log(&dir, 10)
            .into_iter()
            .find(|c| c.summary == "seed")
            .expect("the fixture always creates a commit described \"seed\"");
        let lines = commit_diff(&dir, &seed.id);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(
            text.contains(&"weird{b}.txt"),
            "the renamed file must have a header entry in the commit diff at all: {text:?}"
        );
        assert!(
            text.contains(&"line five"),
            "the origin's content must be readable from the move-from path: {text:?}"
        );
        assert!(
            text.contains(&"line five extended"),
            "the destination's content must be present too: {text:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other shape `--summary`'s compression cannot resolve: a filename containing the literal
    /// `" => "` separator itself. Before this fix, `split_rename` read the file's own text as the
    /// compression markers and handed back a path that does not exist on disk — for a rename of
    /// `arrow => src.txt` to `plaindest.txt`, it produced `from="arrow"`, `to="src.txt =>
    /// plaindest.txt"`. Reading `-T DIFF_TEMPLATE` prints the real source and target verbatim, so no
    /// such path can ever be synthesized.
    #[test]
    fn rename_with_the_literal_arrow_in_the_name_never_synthesizes_a_bogus_path() {
        let Some(dir) = scratch_repo_with_ambiguous_renames("arrow") else {
            return;
        };
        let seed = log(&dir, 10)
            .into_iter()
            .find(|c| c.summary == "seed")
            .expect("the fixture always creates a commit described \"seed\"");
        let lines = commit_diff(&dir, &seed.id);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(
            text.contains(&"plaindest.txt"),
            "the real destination must have a header entry: {text:?}"
        );
        assert!(
            !text.iter().any(|t| t.contains("src.txt => plaindest.txt")),
            "no entry may carry a path that does not exist on disk — this is the bug-shape \
             mangled destination `split_rename` used to synthesize: {text:?}"
        );
        assert!(
            !text.contains(&"arrow"),
            "no entry may carry the bug-shape mangled origin either: {text:?}"
        );
        assert!(
            text.contains(&"line five"),
            "the origin's content (read from \"arrow => src.txt\") must be readable: {text:?}"
        );
        assert!(
            text.contains(&"line five extended"),
            "the destination's content must be present too: {text:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
