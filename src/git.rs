// Fetching git status (FR-7). The data source for coloring each tree row's M/A/U/deleted, etc.
// Even without the `git` feature (on by default), `statuses()` returns an empty map, so everything
// besides the feature itself keeps working.
//
// Spec: fetch the whole repository's status at once, into a (absolute path → kind) map.
// A directory reflects the most important change among its descendants, folded in (rollup).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::style::Color;

/// Test-only counter of actual `statuses()` invocations, so a speed guard can assert the per-workdir
/// status cache reuses the result across `h`/`l` instead of re-scanning the whole worktree.
/// Only the `git`-feature `statuses` increments it and the guard is `git`-gated, so scope it to both.
#[cfg(all(test, feature = "git"))]
pub static STATUS_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(feature = "git")]
thread_local! {
    /// Whether git integration (`[external] git`) is allowed to run **on this thread**. Deliberately
    /// thread-local rather than a process-wide global: `App::new` sets it once for the thread that
    /// constructs the App (production's single main thread, or — in tests — each test's own thread,
    /// since the default test harness runs every `#[test]` on a fresh thread). That keeps concurrently
    /// running tests from ever observing each other's `external.git = false` (a shared `AtomicBool`
    /// toggled per-test caused exactly this kind of flake before — see `STATUS_CALLS`'s history/memory
    /// `git-watch-feedback-loop-perf`). The two places that hop to a **new** background thread
    /// (`spawn_or_sync_statuses`/`spawn_or_sync_ignored` in `app/bookmark_actions.rs`) only ever spawn
    /// when `workdir()` — itself gated below — returned `Some`, i.e. only when this was already `true`,
    /// so the spawned thread's default value (`true`) is already correct without extra propagation.
    static EXTERNAL_GIT_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Enable/disable git integration on the current thread (config `[external] git`). Called once by
/// `App::new`. When disabled, every function below returns exactly the value it would return with the
/// `git` feature compiled out (empty map/set/Vec, `None`, or `Err`) — downstream code already treats
/// that as "not a repo" and degrades safely (flash, no colors, etc.), so nothing else needs to change.
#[cfg(feature = "git")]
pub fn set_external_git_enabled(enabled: bool) {
    EXTERNAL_GIT_ENABLED.with(|c| c.set(enabled));
}

/// Whether git integration is enabled on the current thread: the config allows it **and** a git
/// executable exists on this machine. Defaults to `true` (matches every existing test/binary that
/// never calls `set_external_git_enabled`) as long as `git --version` succeeds.
#[cfg(feature = "git")]
pub fn external_git_enabled() -> bool {
    EXTERNAL_GIT_ENABLED.with(|c| c.get()) && git_binary_available()
}

/// Probe result of `git --version`, cached process-wide. Unlike the config flag above this cannot
/// differ per thread (the machine either has git or it does not), so a `OnceLock` is right here.
#[cfg(feature = "git")]
static GIT_BINARY_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(all(test, feature = "git"))]
thread_local! {
    /// Test-only override of the probe. A `OnceLock` cannot be reset, so tests that need to simulate
    /// "this machine has no git" set this instead. Thread-local for the same reason as
    /// `EXTERNAL_GIT_ENABLED`: parallel tests must never observe each other's override.
    static GIT_BINARY_AVAILABLE_OVERRIDE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// Force `git_binary_available()` to a value on the current thread (`None` = probe for real).
/// Tests must reset it to `None` when done.
#[cfg(all(test, feature = "git"))]
pub fn set_git_binary_available_for_test(v: Option<bool>) {
    GIT_BINARY_AVAILABLE_OVERRIDE.with(|c| c.set(v));
}

/// Whether a usable `git` executable exists on this machine (`git --version` succeeds).
///
/// Probed **once, lazily** — on first use rather than at startup. konoma only shells out to git
/// inside a repository (repository discovery itself is in-process libgit2), and on macOS without the
/// Xcode Command Line Tools `/usr/bin/git` is a stub whose invocation pops a system "install the
/// developer tools" dialog; probing eagerly would raise that dialog for users who merely opened a
/// directory that is not a repository.
#[cfg(feature = "git")]
pub fn git_binary_available() -> bool {
    #[cfg(test)]
    if let Some(v) = GIT_BINARY_AVAILABLE_OVERRIDE.with(|c| c.get()) {
        return v;
    }
    *GIT_BINARY_AVAILABLE.get_or_init(|| {
        // Discard stdin/stdout/stderr entirely: don't let the version string dirty the TUI's raw
        // mode screen.
        std::process::Command::new("git")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false) // the executable is missing / no execute permission
    })
}

/// Git status of a single file (or a directory rollup).
/// When the `git` feature is disabled the status is always empty, so no variant is ever constructed (warning suppressed).
#[cfg_attr(not(feature = "git"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
    TypeChange,
    Conflicted,
}

impl FileStatus {
    /// Single-character marker shown at the start of the line.
    pub fn marker(self) -> char {
        match self {
            FileStatus::Modified => 'M',
            FileStatus::Added => 'A',
            FileStatus::Untracked => 'U',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::TypeChange => 'T',
            FileStatus::Conflicted => '!',
        }
    }

    /// Status color (color = meaning).
    pub fn color(self) -> Color {
        match self {
            FileStatus::Modified => Color::Yellow,
            FileStatus::Added => Color::Green, // added (staged) = green
            FileStatus::Untracked => Color::LightGreen, // untracked = a lighter green (distinct from Added)
            FileStatus::Deleted => Color::Red,
            FileStatus::Renamed => Color::Cyan,
            FileStatus::TypeChange => Color::Yellow,
            FileStatus::Conflicted => Color::LightRed,
        }
    }

    /// Priority when rolling up into a directory (higher = more important = represented by that color).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn rank(self) -> u8 {
        match self {
            FileStatus::Conflicted => 6,
            FileStatus::Deleted => 5,
            FileStatus::Modified => 4,
            FileStatus::Renamed => 3,
            FileStatus::Added => 2,
            FileStatus::TypeChange => 1,
            FileStatus::Untracked => 0,
        }
    }
}

/// Returns the repository status as (absolute path -> kind). Discovers the repo containing `root`.
/// Returns an empty map (= no markers) when not a repo / on failure / when the feature is disabled.
///
/// **Computation is delegated to the `git status` CLI.** git2's (libgit2) `repo.statuses()` is
/// orders of magnitude slower on working trees holding huge ignored trees (target/build, etc.) —
/// measured: on EarthApp (41GB), libgit2 ≈600ms vs `git status` CLI ≈20ms (about 30x). Because it ran
/// synchronously at startup and on every watch refresh, it was the main cause of UI freezes.
/// Only repo discovery uses git2 (discover is lightweight because it does not compute status).
#[cfg(feature = "git")]
pub fn statuses(root: &Path) -> HashMap<PathBuf, FileStatus> {
    if !external_git_enabled() {
        return HashMap::new();
    }
    #[cfg(test)]
    STATUS_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    use std::os::unix::ffi::OsStrExt;
    let mut map = HashMap::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return map; // not a repo / a bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // porcelain v1 -z: each record is `XY <path>`, NUL-separated. When it's a rename/copy (X or Y
    // is R/C), the NUL field right after is the old path. -uall = recurse into untracked dirs too
    // (equivalent to git2's recurse_untracked_dirs), --ignored=no = exclude ignored, and
    // -c status.renames=true forces rename detection (emits R regardless of the user's config).
    let out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // The etiquette for background tools: don't take an optional
            // lock (which writes back the index.lock stat cache). Keeps konoma's own status scans
            // from making a `git pull` fail with "index.lock: File exists" (git 2.15+).
            "-c",
            "status.renames=true",
            "status",
            "--porcelain=v1",
            "-z",
            "-uall",
            "--ignored=no",
        ])
        .output();
    let Ok(out) = out else {
        return map;
    };
    if !out.status.success() {
        return map;
    }

    let mut fields = out.stdout.split(|&b| b == 0);
    while let Some(rec) = fields.next() {
        // Each record is `XY` + a separating space + the path. Discard fractions (a trailing
        // empty field, etc.).
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        let is_rename = x == b'R' || x == b'C' || y == b'R' || y == b'C';
        if is_rename {
            // Consume and discard the old-path field (the status attaches to the new path =
            // the current record).
            let _ = fields.next();
        }
        let rel = PathBuf::from(std::ffi::OsStr::from_bytes(&rec[3..]));
        let abs = workdir.join(rel);
        rollup(&mut map, &workdir, &abs, classify_porcelain(x, y));
    }
    map
}

/// Maps porcelain v1's two XY characters to a FileStatus. Aligned to the same priority order as the git2-based `classify`.
#[cfg(feature = "git")]
fn classify_porcelain(x: u8, y: u8) -> FileStatus {
    if x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D') {
        FileStatus::Conflicted // unmerged (conflict)
    } else if x == b'?' {
        FileStatus::Untracked // "??"
    } else if x == b'D' || y == b'D' {
        FileStatus::Deleted
    } else if x == b'A' {
        FileStatus::Added
    } else if x == b'R' || y == b'R' || x == b'C' || y == b'C' {
        FileStatus::Renamed
    } else if x == b'M' || y == b'M' {
        FileStatus::Modified
    } else if x == b'T' || y == b'T' {
        FileStatus::TypeChange
    } else {
        FileStatus::Modified
    }
}

#[cfg(not(feature = "git"))]
pub fn statuses(_root: &Path) -> HashMap<PathBuf, FileStatus> {
    HashMap::new()
}

/// Returns the set of absolute paths of the **top-level entries** (files/directories) excluded by gitignore.
/// Because of `recurse_ignored_dirs(false)`, `node_modules` and the like are not enumerated; only
/// **that one directory** is included (avoids enumerating huge trees). Used to dim tree entries whose
/// "self or an ancestor is in this set."
/// Returns an empty set when not a repo / on failure / when the feature is disabled.
///
/// **Computation is delegated to the `git status` CLI** (same reason as `statuses()`). git2's `repo.statuses()`
/// is orders of magnitude slower when scanning the ignored tree on repos holding huge working trees
/// (target/node_modules), and because it ran synchronously inside `tree::render`'s draw closure it was the
/// main cause of first-paint freezes. Only repo discovery uses git2 (discover is lightweight because it does not compute status).
#[cfg(feature = "git")]
pub fn ignored(root: &Path) -> HashSet<PathBuf> {
    if !external_git_enabled() {
        return HashSet::new();
    }
    use std::os::unix::ffi::OsStrExt;
    let mut set = HashSet::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return set; // not a repo / a bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // `--ignored=traditional`: enumerate ignored entries with `!!`, and **collapse a fully-ignored
    //   directory** (node_modules/ becomes one entry, not recursed into) = the same as git2's
    //   recurse_ignored_dirs(false).
    // `-unormal`: pin down the default mode so untracked dirs also collapse (if the user's
    //   status.showUntrackedFiles is `all`, ignored dirs get recursively expanded and collapse
    //   breaks, so this is fixed here). `??` lines are discarded below.
    let out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // no optional locks (same reason as statuses())
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored=traditional",
            "-unormal",
        ])
        .output();
    let Ok(out) = out else {
        return set;
    };
    if !out.status.success() {
        return set;
    }

    for rec in out.stdout.split(|&b| b == 0) {
        // An ignored entry is `!! <path>`. Other records like a change/untracked (`??`) are discarded.
        if rec.len() < 4 || rec[0] != b'!' || rec[1] != b'!' {
            continue;
        }
        // An ignored directory comes with a trailing `/` → strip it so it matches the tree's absolute path.
        let mut raw = &rec[3..];
        while raw.last() == Some(&b'/') {
            raw = &raw[..raw.len() - 1];
        }
        set.insert(workdir.join(Path::new(std::ffi::OsStr::from_bytes(raw))));
    }
    set
}

#[cfg(not(feature = "git"))]
pub fn ignored(_root: &Path) -> HashSet<PathBuf> {
    HashSet::new()
}

/// Returns the **workdir (working-tree root)** of the git repository that `root` belongs to (canonicalized).
/// Returns None when not a repo / when the feature is disabled. `discover` is lightweight because it does not
/// compute status, so it serves as the key for deciding "do not rebuild the expensive `ignored` set when
/// moving root within the same repository."
#[cfg(feature = "git")]
pub fn workdir(root: &Path) -> Option<PathBuf> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let wd = repo.workdir()?.to_path_buf();
    Some(wd.canonicalize().unwrap_or(wd))
}

#[cfg(not(feature = "git"))]
pub fn workdir(_root: &Path) -> Option<PathBuf> {
    None
}

/// Returns the **git directory** (where `HEAD`/`index`/refs actually live) for the repository that
/// `root` belongs to (canonicalized). For an ordinary repository this is `<repo>/.git/`. For a
/// **linked worktree** (created with `git worktree add`) it is instead
/// `<main-repo>/.git/worktrees/<name>/` — that linked worktree's own `HEAD` and `index` live there,
/// not under the worktree's checkout. This is a different path from [`workdir`], which always
/// returns the *checked-out tree* side (for a linked worktree, that is the worktree's own root, not
/// the main repo's root). Returns None when not a repo / when the feature is disabled.
#[cfg(feature = "git")]
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let p = repo.path().to_path_buf();
    Some(p.canonicalize().unwrap_or(p))
}

#[cfg(not(feature = "git"))]
pub fn git_dir(_root: &Path) -> Option<PathBuf> {
    None
}

/// Returns the current branch name. A short hash when detached, or the HEAD reference name before the first commit (unborn).
/// Returns None when not a repo / when the feature is disabled.
#[cfg(feature = "git")]
pub fn branch(root: &Path) -> Option<String> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    if let Ok(head) = repo.head() {
        // git2 0.21: shorthand() is Result<&str, Error> (UTF-8 validated). The None-equivalent is
        // absorbed with .ok().
        return head.shorthand().ok().map(|s| s.to_string());
    }
    // If there is no commit yet (unborn), pull it from HEAD's symbolic reference instead.
    // git2 0.21: symbolic_target() is Result<Option<&str>, Error>.
    repo.find_reference("HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(|t| t.to_string()))
        .map(|t| t.trim_start_matches("refs/heads/").to_string())
}

#[cfg(not(feature = "git"))]
pub fn branch(_root: &Path) -> Option<String> {
    None
}

/// The name of the repository a **linked worktree** (`git worktree add`) belongs to, or `None`
/// when `root` is not inside one (including: not a repo, the repo's own main worktree, or the
/// feature/config-disabled build). Nothing besides this is shown for the main worktree — the
/// asymmetry (shown only in a linked worktree) is itself the information: konoma's tree/preview
/// title already shows the root directory's own name, which for a linked worktree is some
/// unrelated branch/feature name, not the project's.
///
/// `repo.is_worktree()` answers "linked worktree" directly (true even from a subdirectory reached
/// by descending with `l`, since discovery walks up to the repo regardless of `root`). The origin
/// name is derived from `repo.commondir()` (the shared git-dir every worktree of one repository
/// points at), whose shape differs by layout:
/// - ordinary repo + its linked worktrees: commondir = `<main>/.git/` → the origin name is the
///   **parent directory's** name (`<main>`'s basename).
/// - bare repo + its worktrees (`git clone --bare` + `git worktree add`): commondir = `<bare>/repo.git/`
///   itself (there is no `.git` **inside** a bare repo) → the origin name is commondir's own
///   basename with a trailing `.git` stripped.
///
/// The two are told apart by whether commondir's basename is literally `.git`.
#[cfg(feature = "git")]
pub fn worktree_origin(root: &Path) -> Option<String> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    if !repo.is_worktree() {
        return None; // the main worktree (or a bare repo with no worktree at all)
    }
    let commondir = repo.commondir();
    let name = commondir.file_name()?.to_str()?;
    if name == ".git" {
        commondir
            .parent()?
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    } else {
        Some(name.strip_suffix(".git").unwrap_or(name).to_string())
    }
}

#[cfg(not(feature = "git"))]
pub fn worktree_origin(_root: &Path) -> Option<String> {
    None
}

// ── Read+write API for diff / changed-files / log (for the Git view) ──────────
//
// Reads go through git2; writes (stage/unstage/discard/commit) are delegated to the `git` CLI
// (to respect hooks, GPG signing, and user config — git2 is never used for writing).
// When the `git` feature is disabled everything returns empty/None/Err, and everything besides
// the feature itself keeps working as normal.

/// Kind of a single diff line. Context = unchanged / Added = addition (green) / Removed = deletion (red).
// The variants are only constructed on the git-feature diff path; the type is still referenced by the
// shared diff renderer under `--no-default-features`, so allow the no-git "never constructed" warning.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// A single diff line. `old_no`/`new_no` are the line numbers in the old/new file (None on the missing side).
/// `text` is the line content (trailing newline removed; the leading +/- is not included).
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

/// One entry in the changed-files list. `path` is absolute; `staged` indicates whether there are INDEX_* changes.
#[derive(Debug, Clone)]
pub struct ChangeEntry {
    pub path: PathBuf,
    pub status: FileStatus,
    pub staged: bool,
}

/// Commit information for one log entry.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub short: String,
    pub summary: String,
    pub author: String,
    pub time_epoch: i64,
}

/// A single local branch (for the `b` branch list). `is_current` = currently checked out.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
}

/// Returns local branches in ascending name order (the current branch has `is_current`).
/// Returns an empty Vec when not a repo / when the feature is disabled / before the first commit (unborn = no branch created yet).
#[cfg(feature = "git")]
pub fn branches(root: &Path) -> Vec<BranchInfo> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Ok(iter) = repo.branches(Some(git2::BranchType::Local)) {
        for (branch, _) in iter.flatten() {
            let is_current = branch.is_head();
            if let Ok(Some(name)) = branch.name() {
                out.push(BranchInfo {
                    name: name.to_string(),
                    is_current,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(not(feature = "git"))]
pub fn branches(_root: &Path) -> Vec<BranchInfo> {
    Vec::new()
}

/// One entry from `git worktree list` — **a linked working tree** (created with `git worktree add`),
/// not to be confused with the rest of this codebase's "worktree" (always "the working tree = the
/// uncommitted changes", e.g. `worktree_diff`/`GraphRow.worktree`/config's `git_worktree_diff`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// Absolute, canonicalized checkout root.
    pub path: PathBuf,
    /// Checked-out branch name (`refs/heads/` stripped). `None` when detached (or, for a bare main
    /// worktree, when there is no checkout at all).
    pub branch: Option<String>,
    /// Short (7-char) HEAD hash. `None` for a bare main worktree, or a linked worktree created
    /// before the repository had its first commit (git reports the all-zero OID there).
    pub head: Option<String>,
    /// Whether this is the worktree the caller (`root`) is presently inside.
    pub is_current: bool,
    /// Whether this is the **main** worktree (always the first record `git worktree list` reports,
    /// regardless of which worktree's directory it is invoked from).
    pub is_main: bool,
    /// Whether this is a bare repository standing in as the main worktree (`git clone --bare` +
    /// `git worktree add`) — it has no checkout of its own to switch into.
    pub is_bare: bool,
    /// `Some(reason)` when locked (`git worktree lock`); `Some("")` when locked with no reason given.
    pub locked: Option<String>,
    /// Whether git considers this entry prunable (its checkout directory is missing/moved).
    pub prunable: bool,
}

/// Returns every worktree linked to the repository containing `root` — the **main** worktree first,
/// then any **linked** worktrees created with `git worktree add` — parsed from
/// `git worktree list --porcelain -z`. Returns an empty Vec when not a repo / on failure / when the
/// feature is disabled (the caller treats that the same as "no repository", same as `branches()`).
///
/// **Computation is delegated to the `git worktree` CLI**, not git2/libgit2: git2's
/// `Repository::worktrees()` only returns the *names* of linked worktrees (never the main one, and
/// not their branch/HEAD/lock state) — matching this function's shape would take several more git2
/// calls per entry. `--porcelain -z` gives the whole picture (path/HEAD/branch/detached/bare/locked/
/// prunable) in one call: the same "heavy or structured read → CLI" split as `statuses()`/`ignored()`.
///
/// **`-z` record shape** (verified against git 2.54.0): each field is one NUL-terminated "line"
/// (`worktree <path>`, then any of `HEAD <sha>`, `branch refs/heads/<name>` XOR `detached`, `bare`,
/// `locked [<reason>]`, `prunable [<reason>]`), and a record ends with one *extra* NUL — two NULs in
/// a row mark the boundary between records, the `-z` equivalent of the blank line that separates
/// records in the non-`-z` porcelain output. `-z` is not optional here: without it a worktree path
/// containing a literal newline would be misparsed (same reasoning as `-z` on `status`). The **first**
/// record is always the main worktree, regardless of which worktree's directory git is invoked from.
/// A bare-repo-as-main-worktree record has only `worktree` + `bare` fields (no `HEAD`/`branch`) — no
/// checkout exists there to switch into.
#[cfg(feature = "git")]
pub fn worktrees(root: &Path) -> Vec<WorktreeInfo> {
    if !external_git_enabled() {
        return Vec::new();
    }
    // `git worktree list` returns the same listing no matter which of the repo's worktrees it's
    // invoked from, so this is just picking *some* directory to run git in — not "the main
    // worktree's workdir". If `root` is inside a linked worktree, this resolves to that linked
    // worktree's own path, not the main one.
    let Some(cwd) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return Vec::new(); // not a repo / a bare repo opened directly as root
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);

    let out = std::process::Command::new("git")
        .current_dir(&cwd)
        .args([
            "--no-optional-locks", // read-only background call (same etiquette as statuses()/ignored())
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }

    // The worktree this konoma tab is presently inside (compared against each record's path below
    // to compute `is_current`). Not necessarily equal to `root` itself: `root` can be a subdirectory
    // of a worktree's checkout.
    let current = workdir(root);

    let mut list: Vec<WorktreeInfo> = Vec::new();
    let mut rec: Vec<&[u8]> = Vec::new();
    for field in out.stdout.split(|&b| b == 0) {
        if field.is_empty() {
            if !rec.is_empty() {
                let is_main = list.is_empty();
                list.push(worktree_from_record(&rec, is_main, current.as_deref()));
                rec.clear();
            }
            continue;
        }
        rec.push(field);
    }
    if !rec.is_empty() {
        let is_main = list.is_empty();
        list.push(worktree_from_record(&rec, is_main, current.as_deref()));
    }
    list
}

/// Parses one `git worktree list --porcelain -z` record (its NUL-separated field lines, `worktree
/// <path>` always first) into a `WorktreeInfo`. `is_main` = this is the first record in the whole
/// listing. `current` is `workdir(root)` — the checked-out root of whichever worktree the caller is
/// presently inside — compared against this record's (canonicalized) path for `is_current`.
#[cfg(feature = "git")]
fn worktree_from_record(fields: &[&[u8]], is_main: bool, current: Option<&Path>) -> WorktreeInfo {
    use std::os::unix::ffi::OsStrExt;
    let mut path = PathBuf::new();
    let mut head: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut is_bare = false;
    let mut locked: Option<String> = None;
    let mut prunable = false;

    for &f in fields {
        if let Some(rest) = f.strip_prefix(b"worktree ") {
            // Raw bytes, not a lossy UTF-8 conversion: a checkout path could in principle hold
            // non-UTF-8 bytes (same care as `statuses()`'s path handling).
            path = PathBuf::from(std::ffi::OsStr::from_bytes(rest));
            continue;
        }
        // Every other tag line is git-generated ASCII (HEAD/branch/detached/bare/locked/prunable),
        // so a lossy UTF-8 view is safe here — only the path above needs raw-byte handling.
        let line = String::from_utf8_lossy(f);
        if let Some(rest) = line.strip_prefix("HEAD ") {
            // An unborn linked worktree (its branch was created before the repository had any
            // commit) reports the all-zero OID; treat that the same as "no HEAD yet", not a real
            // short hash.
            if !rest.bytes().all(|b| b == b'0') {
                head = Some(rest.chars().take(7).collect());
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            branch = Some(
                rest.strip_prefix("refs/heads/")
                    .map(str::to_string)
                    .unwrap_or_else(|| rest.to_string()),
            );
        } else if line == "bare" {
            is_bare = true;
        } else if line == "locked" {
            locked = Some(String::new());
        } else if let Some(rest) = line.strip_prefix("locked ") {
            locked = Some(rest.to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            prunable = true;
        }
        // "detached" itself carries no data beyond "branch is absent" (already the default).
    }

    let path = path.canonicalize().unwrap_or(path);
    let is_current = current.is_some_and(|c| c == path);
    WorktreeInfo {
        path,
        branch,
        head,
        is_current,
        is_main,
        is_bare,
        locked,
        prunable,
    }
}

#[cfg(not(feature = "git"))]
pub fn worktrees(_root: &Path) -> Vec<WorktreeInfo> {
    Vec::new()
}

/// For selecting which branches to show in the graph: returns local branches **ordered by most-recent tip commit**.
/// `(name, is_current, time_epoch)`. Ordered so the cap (`ui.graph_max_branches`) can take from the front.
#[cfg(feature = "git")]
pub fn branches_by_recency(root: &Path) -> Vec<(String, bool, i64)> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool, i64)> = Vec::new();
    if let Ok(iter) = repo.branches(Some(git2::BranchType::Local)) {
        for (branch, _) in iter.flatten() {
            let is_current = branch.is_head();
            let Ok(Some(name)) = branch.name() else {
                continue;
            };
            // The tip commit's time (0 = treated as oldest if it can't be fetched).
            let t = branch
                .get()
                .peel_to_commit()
                .map(|c| c.time().seconds())
                .unwrap_or(0);
            out.push((name.to_string(), is_current, t));
        }
    }
    // Newest first (descending). Same-time entries are stabilized by name.
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn branches_by_recency(_root: &Path) -> Vec<(String, bool, i64)> {
    Vec::new()
}

/// The tip commit OID of local branch `name` (used to pin the base branch to lane0).
/// Returns None when not a repo / when the branch does not exist / when the feature is disabled.
#[cfg(feature = "git")]
pub fn branch_tip(root: &Path, name: &str) -> Option<String> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let branch = repo.find_branch(name, git2::BranchType::Local).ok()?;
    let oid = branch.get().peel_to_commit().ok()?.id();
    Some(oid.to_string())
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn branch_tip(_root: &Path, _name: &str) -> Option<String> {
    None
}

/// One graph legend entry: branch name + its lane color + HEAD/base flags.
#[derive(Debug, Clone)]
pub struct LegendEntry {
    pub name: String,
    pub color: ratatui::style::Color,
    pub is_head: bool,
    pub is_base: bool,
}

/// Builds a "visible local branch -> lane color" legend from already-rendered graph rows.
/// Takes the color of each branch tip row's node (`●`/`◆`). Remotes/tags are excluded (local branch names only).
/// Order is **HEAD first, base next, then appearance order (topo = newest first)**.
#[cfg(feature = "git")]
pub fn legend_from_rows(
    rows: &[GraphRow],
    root: &Path,
    base_label: Option<&str>,
) -> Vec<LegendEntry> {
    use ratatui::style::Color;
    use std::collections::{HashMap, HashSet};
    let locals = branches(root);
    let local_names: HashSet<&str> = locals.iter().map(|b| b.name.as_str()).collect();
    let head_branch = locals.iter().find(|b| b.is_current).map(|b| b.name.clone());

    let mut order: Vec<String> = Vec::new();
    let mut color_of: HashMap<String, Color> = HashMap::new();
    for r in rows {
        if r.commit.is_none() || r.refs.is_empty() {
            continue;
        }
        let node_color = r
            .graph
            .iter()
            .find(|(g, _)| g == "●" || g == "◆")
            .and_then(|(_, st)| st.fg)
            .unwrap_or(Color::Reset);
        for tok in r.refs.split(',') {
            let tok = tok.trim();
            let name = tok.strip_prefix("HEAD -> ").unwrap_or(tok);
            if name == "HEAD" || name.starts_with("tag:") || !local_names.contains(name) {
                continue;
            }
            if color_of.contains_key(name) {
                continue;
            }
            color_of.insert(name.to_string(), node_color);
            order.push(name.to_string());
        }
    }
    let mut out: Vec<LegendEntry> = order
        .into_iter()
        .map(|name| {
            let is_head = head_branch.as_deref() == Some(name.as_str());
            let is_base = base_label == Some(name.as_str());
            let color = color_of.get(&name).copied().unwrap_or(Color::Reset);
            LegendEntry {
                name,
                color,
                is_head,
                is_base,
            }
        })
        .collect();
    // Stable sort: HEAD (first) → base → the rest keep their appearance order.
    out.sort_by_key(|e| (!e.is_head, !e.is_base));
    out
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn legend_from_rows(
    _rows: &[GraphRow],
    _root: &Path,
    _base_label: Option<&str>,
) -> Vec<LegendEntry> {
    Vec::new()
}

/// Returns the working-tree diff of `file` (HEAD tree -> workdir, including the index) line by line.
/// Untracked files are treated as all-Added lines, as is the no-HEAD (unborn) case.
/// Returns an empty Vec when not a repo / on failure / when the feature is disabled. Never panics.
#[cfg(feature = "git")]
pub fn file_diff(root: &Path, file: &Path) -> Vec<DiffLine> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let Some(workdir) = repo.workdir() else {
        return Vec::new();
    };
    let workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    // The pathspec is passed as workdir-relative. Even for an absolute path, try to make it
    // relative via strip_prefix.
    let file_abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = file_abs
        .strip_prefix(&workdir)
        .unwrap_or(&file_abs)
        .to_path_buf();
    let rel_str = rel.to_string_lossy().to_string();

    // The HEAD tree (None = treated as an empty tree if unborn).
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        // Also emit line content for untracked files (by default only the "untracked" fact
        // shows, with no line diff).
        .show_untracked_content(true)
        .pathspec(&rel_str);

    let diff = match repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    collect_diff_lines(&diff, false)
}

#[cfg(not(feature = "git"))]
pub fn file_diff(_root: &Path, _file: &Path) -> Vec<DiffLine> {
    Vec::new()
}

/// Line-level diff between two in-memory contents (`old` → `new`), producing the same `DiffLine`
/// shape as `file_diff` so the existing GitDiff renderer consumes it unchanged. Used by the follow
/// baseline diff, where `old` is the file's content at follow-start and `new` is its content now.
/// Emits hunks with 3 lines of context (like git), not the whole file. Pure text — no git needed,
/// so it is available in the no-git build too.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn diff_contents(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for group in diff.grouped_ops(3) {
        for op in &group {
            for change in diff.iter_changes(op) {
                let raw = change.value();
                // Drop the trailing newline (DiffLine.text's contract is that it never includes
                // one, same as file_diff).
                let text = raw
                    .strip_suffix('\n')
                    .unwrap_or(raw)
                    .strip_suffix('\r')
                    .unwrap_or_else(|| raw.strip_suffix('\n').unwrap_or(raw))
                    .to_string();
                let old_no = change.old_index().map(|i| i as u32 + 1);
                let new_no = change.new_index().map(|i| i as u32 + 1);
                let kind = match change.tag() {
                    similar::ChangeTag::Equal => DiffLineKind::Context,
                    similar::ChangeTag::Delete => DiffLineKind::Removed,
                    similar::ChangeTag::Insert => DiffLineKind::Added,
                };
                out.push(DiffLine {
                    kind,
                    old_no,
                    new_no,
                    text,
                });
            }
        }
    }
    out
}

/// The current HEAD commit's full SHA (pinned at follow-start so a clean-at-follow-start file's
/// baseline stays fixed even if the agent commits mid-session). None if unborn / not a repo.
#[cfg(feature = "git")]
pub fn head_commit_id(root: &Path) -> Option<String> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

/// The bytes of `file` as of commit `sha` (the baseline for a file that was clean at follow-start).
/// None if the commit/path is unresolvable (e.g. the file did not exist then → caller treats it as
/// all-added). `file` is absolute; it is made relative to the repo workdir for the tree lookup.
#[cfg(feature = "git")]
pub fn blob_at(root: &Path, sha: &str, file: &Path) -> Option<Vec<u8>> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let workdir = repo.workdir()?;
    let workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    let file_abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = file_abs.strip_prefix(&workdir).ok()?;
    let oid = git2::Oid::from_str(sha).ok()?;
    let tree = repo.find_commit(oid).ok()?.tree().ok()?;
    let entry = tree.get_path(rel).ok()?;
    let obj = entry.to_object(&repo).ok()?;
    Some(obj.as_blob()?.content().to_vec())
}

/// Walks a git2 Diff and assembles the sequence of DiffLines.
/// When `with_headers` = true, inserts a Context line at each file boundary (text = bare path, line numbers None) (for commit_diff). The renderer turns it into a boxed header.
#[cfg(feature = "git")]
fn collect_diff_lines(diff: &git2::Diff, with_headers: bool) -> Vec<DiffLine> {
    use std::cell::RefCell;
    let lines: RefCell<Vec<DiffLine>> = RefCell::new(Vec::new());
    let last_file: RefCell<Option<String>> = RefCell::new(None);

    let _ = diff.foreach(
        &mut |delta, _| {
            if with_headers {
                let path = delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut lf = last_file.borrow_mut();
                if lf.as_deref() != Some(path.as_str()) {
                    // A file-boundary header: text = **the bare path**, both line numbers None
                    // (= the marker that it's a header). The render side (gitdiff) turns it into a
                    // boxed header and switches per-file highlighting by extension.
                    lines.borrow_mut().push(DiffLine {
                        kind: DiffLineKind::Context,
                        old_no: None,
                        new_no: None,
                        text: path.clone(),
                    });
                    *lf = Some(path);
                }
            }
            true
        },
        None,
        None,
        Some(&mut |_delta, _hunk, line| {
            let kind = match line.origin() {
                '+' => DiffLineKind::Added,
                '-' => DiffLineKind::Removed,
                _ => DiffLineKind::Context,
            };
            let text = String::from_utf8_lossy(line.content())
                .trim_end_matches(['\n', '\r'])
                .to_string();
            lines.borrow_mut().push(DiffLine {
                kind,
                old_no: line.old_lineno(),
                new_no: line.new_lineno(),
                text,
            });
            true
        }),
    );
    lines.into_inner()
}

/// Returns the list of changed files (including untracked) as one entry per file, sorted by path.
///
/// **Computation is delegated to the `git status` CLI** (same reason as `statuses()`). git2's (libgit2)
/// `repo.statuses()` is orders of magnitude slower on working trees holding huge ignored trees (target/build, etc.) —
/// measured: on EarthApp (41GB), libgit2 ≈600ms vs CLI ≈20ms. Because it ran synchronously every time the changes hub (`o`)
/// was opened, this cost showed up each time. Only repo discovery uses git2 (discover is lightweight because it does not compute status).
#[cfg(feature = "git")]
pub fn changed_files(root: &Path) -> Vec<ChangeEntry> {
    if !external_git_enabled() {
        return Vec::new();
    }
    use std::os::unix::ffi::OsStrExt;
    let mut out = Vec::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return out; // not a repo / a bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // porcelain v1 -z: each record is `XY <path>`, NUL-separated. When it's a rename/copy (X or Y
    // is R/C), the NUL field right after is the old path (the new path comes first — confirmed by
    // measurement). -uall = recurse into untracked dirs too, --ignored=no = exclude ignored, and
    // -c status.renames=true forces rename detection (emits R regardless of the user's config).
    let cmd_out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // no optional locks (same reason as statuses())
            "-c",
            "status.renames=true",
            "status",
            "--porcelain=v1",
            "-z",
            "-uall",
            "--ignored=no",
        ])
        .output();
    let Ok(cmd_out) = cmd_out else {
        return out;
    };
    if !cmd_out.status.success() {
        return out;
    }

    let mut fields = cmd_out.stdout.split(|&b| b == 0);
    while let Some(rec) = fields.next() {
        // Each record is `XY` + a separating space + the path. Discard fractions (a trailing
        // empty field, etc.).
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        let is_rename = x == b'R' || x == b'C' || y == b'R' || y == b'C';
        if is_rename {
            // Consume and discard the old-path field (the status attaches to the new path =
            // the current record).
            let _ = fields.next();
        }
        // staged = whether the INDEX side (X column) has a change. ' ' = no index change / '?' =
        // untracked / 'U' = unmerged are not treated as staged (equivalent to the git2 version's
        // INDEX_NEW/MODIFIED/DELETED/RENAMED/TYPECHANGE).
        let staged = matches!(x, b'M' | b'A' | b'D' | b'R' | b'C' | b'T');
        let rel = PathBuf::from(std::ffi::OsStr::from_bytes(&rec[3..]));
        out.push(ChangeEntry {
            path: workdir.join(rel),
            status: classify_porcelain(x, y),
            staged,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(not(feature = "git"))]
pub fn changed_files(_root: &Path) -> Vec<ChangeEntry> {
    Vec::new()
}

/// Returns up to `max` commits, newest first, starting from HEAD.
#[cfg(feature = "git")]
pub fn log(root: &Path, max: usize) -> Vec<CommitInfo> {
    let mut out = Vec::new();
    if !external_git_enabled() {
        return out;
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return out;
    };
    let Ok(mut walk) = repo.revwalk() else {
        return out;
    };
    if walk.push_head().is_err() {
        return out; // unborn, etc.
    }
    let _ = walk.set_sorting(git2::Sort::TIME);
    for oid in walk.flatten().take(max) {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let id = oid.to_string();
        let short = id.chars().take(7).collect();
        // git2 0.21: summary() is Result<Option<&str>, Error>. None / non-UTF-8 becomes an empty string.
        let summary = commit.summary().ok().flatten().unwrap_or("").to_string();
        let author = commit.author().name().unwrap_or("").to_string();
        let time_epoch = commit.time().seconds();
        out.push(CommitInfo {
            id,
            short,
            summary,
            author,
            time_epoch,
        });
    }
    out
}

#[cfg(not(feature = "git"))]
pub fn log(_root: &Path, _max: usize) -> Vec<CommitInfo> {
    Vec::new()
}

/// Returns the diff of commit `id` (against its first parent; a root commit diffs against the empty tree) for all files.
/// Inserts a Context header line (bare path, line numbers None) at the start of each file.
#[cfg(feature = "git")]
pub fn commit_diff(root: &Path, id: &str) -> Vec<DiffLine> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let Ok(oid) = git2::Oid::from_str(id) else {
        return Vec::new();
    };
    let Ok(commit) = repo.find_commit(oid) else {
        return Vec::new();
    };
    let Ok(new_tree) = commit.tree() else {
        return Vec::new();
    };
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&new_tree), None) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    collect_diff_lines(&diff, true)
}

/// **Metadata** of a commit (for the heading at the top of the detail view). `message` is the full %B-equivalent text
/// (subject + body, multi-line, trailing newline removed). Shown above the diff on Enter in log/graph.
#[derive(Debug, Clone)]
pub struct CommitMeta {
    /// Full hash (40 digits). Used by the copy feature's "full hash."
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub id: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

/// Returns the metadata of commit `id` (short hash, author, date, full message).
/// Returns None when not a repo / when the commit does not exist / when the feature is disabled.
#[cfg(feature = "git")]
pub fn commit_meta(root: &Path, id: &str) -> Option<CommitMeta> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let oid = git2::Oid::from_str(id).ok()?;
    let commit = repo.find_commit(oid).ok()?;
    let message = commit.message().unwrap_or("").trim_end().to_string();
    let author = commit.author().name().unwrap_or("").to_string();
    let secs = commit.time().seconds().max(0) as u64;
    let date = crate::fileops::format_epoch_short(secs);
    let short = id[..id.len().min(7)].to_string();
    Some(CommitMeta {
        id: id.to_string(),
        short,
        author,
        date,
        message,
    })
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn commit_meta(_root: &Path, _id: &str) -> Option<CommitMeta> {
    None
}

#[cfg(not(feature = "git"))]
pub fn commit_diff(_root: &Path, _id: &str) -> Vec<DiffLine> {
    Vec::new()
}

/// Resolves the target repo workdir for write operations (prefers discover, falls back to root on failure).
#[cfg(feature = "git")]
fn workdir_of(root: &Path) -> PathBuf {
    git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(|w| w.to_path_buf()))
        .unwrap_or_else(|| root.to_path_buf())
}

/// Picks the single line out of `git`'s stderr that's worth showing the user.
///
/// Two facts about how this ends up on screen drive the order here: the flash footer renders
/// the message as **one line**, clipped at the terminal width — an embedded newline doesn't
/// wrap, it just burns width before the useful part ever gets there — and `git` routinely
/// writes progress chatter to stderr *before* the line that actually explains the failure (e.g.
/// `worktree add` prints `Preparing worktree (checking out '<branch>')` ahead of its `fatal:`
/// line). So: prefer a `fatal:`-prefixed line, then `error:`, then fall back to the last
/// non-empty line (covers subcommands whose failure line uses neither prefix). Only when stderr
/// was empty do we synthesize a message from the exit status.
#[cfg(feature = "git")]
fn git_error_message(out: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(l) = lines.iter().find(|l| l.starts_with("fatal:")) {
        return (*l).to_string();
    }
    if let Some(l) = lines.iter().find(|l| l.starts_with("error:")) {
        return (*l).to_string();
    }
    if let Some(l) = lines.last() {
        return (*l).to_string();
    }
    format!("git exited with {}", out.status)
}

/// Runs the `git` CLI in the repo workdir. A non-zero exit returns an Err whose message is
/// `git`'s own `fatal:`/`error:` line (see `git_error_message`) — deliberately *not* the command
/// line that was run: the caller just pressed the key that triggered it, so echoing back what it
/// did only pushes the actual reason further right, off the edge of the flash line.
#[cfg(feature = "git")]
fn run_git(root: &Path, args: &[&str]) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};
    if !external_git_enabled() {
        return Err(anyhow!("git disabled (external.git = false)"));
    }
    let cwd = workdir_of(root);
    let out = std::process::Command::new("git")
        .current_dir(&cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to launch git {}", args.join(" ")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(git_error_message(&out)))
    }
}

/// Stages `file` (git add -- <file>).
#[cfg(feature = "git")]
pub fn stage(root: &Path, file: &Path) -> anyhow::Result<()> {
    run_git(root, &["add", "--", &file.to_string_lossy()])
}

/// Unstages `file` (git reset -q HEAD -- <file>).
#[cfg(feature = "git")]
pub fn unstage(root: &Path, file: &Path) -> anyhow::Result<()> {
    run_git(
        root,
        &["reset", "-q", "HEAD", "--", &file.to_string_lossy()],
    )
}

/// **Stages all** changes (git add -A: modified/new/deleted all go to the index).
#[cfg(feature = "git")]
pub fn stage_all(root: &Path) -> anyhow::Result<()> {
    run_git(root, &["add", "-A"])
}

/// **Unstages everything** (git reset -q HEAD: resets the index to HEAD). Leaves the working tree unchanged.
#[cfg(feature = "git")]
pub fn unstage_all(root: &Path) -> anyhow::Result<()> {
    run_git(root, &["reset", "-q", "HEAD"])
}

/// Discards tracked changes to `file` (git checkout -q -- <file>).
#[cfg(feature = "git")]
pub fn discard(root: &Path, file: &Path) -> anyhow::Result<()> {
    run_git(root, &["checkout", "-q", "--", &file.to_string_lossy()])
}

/// Commits staged changes (git commit -m <message>).
#[cfg(feature = "git")]
pub fn commit(root: &Path, message: &str) -> anyhow::Result<()> {
    run_git(root, &["commit", "-m", message])
}

/// Switches to branch `name` (git switch <name>). git refuses (Err) if uncommitted changes conflict.
/// Uses `switch` rather than `checkout`: it removes the ambiguity between branch and path names and avoids accidental detached HEAD
/// (a switch-only command that does not pull in the file-restore feature). Requires Git >= 2.23.
#[cfg(feature = "git")]
pub fn checkout(root: &Path, name: &str) -> anyhow::Result<()> {
    run_git(root, &["switch", name])
}

/// Creates a new branch and switches to it (git switch -c <name>). git refuses (Err) if the name already exists.
#[cfg(feature = "git")]
pub fn create_branch(root: &Path, name: &str) -> anyhow::Result<()> {
    run_git(root, &["switch", "-c", name])
}

/// Deletes a branch. `force` = false uses `-d` (git refuses unmerged branches = safe) / true uses `-D` (force).
/// git refuses the currently checked-out branch.
#[cfg(feature = "git")]
pub fn delete_branch(root: &Path, name: &str, force: bool) -> anyhow::Result<()> {
    let flag = if force { "-D" } else { "-d" };
    run_git(root, &["branch", flag, name])
}

/// Creates a new **linked worktree** (`git worktree add`) at `path`, checked out to `branch`.
/// `create_branch=true` also creates `branch` fresh (`-b`, starting from HEAD — same starting
/// point as `create_branch()`'s `switch -c`); `create_branch=false` checks out an existing branch
/// (no `-b`). The caller (`App::dialog_submit`'s `PendingOp::WorktreeCreate` arm) decides which by
/// probing `branch_tip` first — this function just executes whichever was decided, it does not
/// re-probe.
///
/// No `--no-optional-locks` here, unlike `worktrees()`/`statuses()`/`ignored()`: that flag is
/// this codebase's etiquette for *background reads* not to contend with a concurrent user-facing
/// write (e.g. so `git pull` never trips on a lock our own idle status scan happens to be
/// holding). `worktree add` is the opposite — a write the user just explicitly triggered by
/// submitting the dialog — so it takes the same lock any other write here does, same as `stage`/
/// `commit`/`checkout`/`create_branch` right above.
///
/// Errors surface as `git`'s own `fatal:` line via `run_git`'s `git_error_message` (same contract
/// as `create_branch`/`checkout` — the caller flashes it as-is). Real cases hit in practice: the
/// target directory already has content (`fatal: '<path>' already exists` — an *empty* directory
/// is fine, git happily uses it), the branch is already checked out by another worktree (`fatal:
/// '<branch>' is already used by worktree at '<path>'`), and — defensively, since the caller
/// already checked via `branch_tip` — `create_branch=true` racing an existing branch name (`fatal:
/// a branch named '<branch>' already exists`).
#[cfg(feature = "git")]
pub fn worktree_add(
    root: &Path,
    path: &Path,
    branch: &str,
    create_branch: bool,
) -> anyhow::Result<()> {
    let path = path.to_string_lossy();
    if create_branch {
        run_git(root, &["worktree", "add", "-b", branch, &path])
    } else {
        run_git(root, &["worktree", "add", &path, branch])
    }
}

/// One row of the commit graph (`G`: SourceTree / Git Graph style). `graph` = the (char, style) sequence of the colored lane area;
/// `commit` = the commit on that row (Some = commit row / None = connector-only row).
#[derive(Debug, Clone)]
pub struct GraphRow {
    pub graph: Vec<(String, ratatui::style::Style)>,
    pub commit: Option<String>,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub refs: String,
    /// Pseudo-row: uncommitted working-tree changes (SourceTree/Git Graph's "Uncommitted changes").
    /// `commit` is None, but the cursor can rest on it and `Enter` opens worktree_diff.
    pub worktree: bool,
}

/// Turns the commit graph into rows by **assigning lanes ourselves from the DAG (parent-ID lists)**.
/// `git log --graph`'s diagonal lines (`/ \`) clash with the box-based, angular TUI, so they are not used;
/// connections use angular box-drawing (`│ ─ ├ ┤ ┌ ┐ └ ┘ ┼` plus nodes `● normal / ◆ merge`).
/// Node/connector colors are **that lane's color** (cyclic palette). **lane0 (base, the left trunk) has a fixed color** so it is always visible.
/// Returns an empty Vec when not a repo / on failure / when the feature is disabled.
///
/// When `base` (the OID of the base branch tip) is given, its **first-parent chain is pinned to lane0 as a straight left line**
/// (Phase 2). With `None` it behaves as before (leftmost by appearance order). Non-base commits branch off to lane1 and beyond (right),
/// and unmerged branches keep their right lane all the way to the top. `docs/GRAPH-BASE-SPEC.md`.
#[cfg(feature = "git")]
pub fn graph_with_base(
    root: &Path,
    base: Option<&str>,
    lang: crate::i18n::Lang,
    refs: Option<&[String]>,
) -> Vec<GraphRow> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let commits = dag_commits(root, 400, refs);
    let wt = worktree_payload(root, lang);
    lay_out_lanes(&commits, base, wt)
}

/// Raw DAG data for drawing the graph (with parent-ID lists). Does not use `--graph`; lays out ourselves.
#[cfg(feature = "git")]
#[derive(Clone)]
struct DagCommit {
    id: String,
    parents: Vec<String>,
    short: String,
    subject: String,
    author: String,
    date: String,
    refs: String,
}

/// Fetches the equivalent of `git log --topo-order --parents`, `%x1f`-delimited, into a sequence of `DagCommit`.
/// Passing `refs` restricts to **only those branches** (for the visibility toggle); `None`/empty uses `--all` (all branches).
#[cfg(feature = "git")]
fn dag_commits(root: &Path, max: usize, refs: Option<&[String]>) -> Vec<DagCommit> {
    let cwd = workdir_of(root);
    // %P = the parent IDs (space-separated). topo-order = never emits a parent before its child +
    // branches don't get mixed (the lanes stay stable).
    let fmt = "--format=%x1f%H%x1f%P%x1f%h%x1f%s%x1f%an%x1f%ad%x1f%D";
    let mut args: Vec<String> = vec![
        "log".into(),
        "--topo-order".into(),
        "--date=short".into(),
        "-n".into(),
        max.to_string(),
        fmt.into(),
    ];
    match refs {
        // Only the given branches. The caller must always include HEAD (to avoid an empty graph).
        Some(r) if !r.is_empty() => args.extend(r.iter().cloned()),
        _ => args.push("--all".into()),
    }
    let out = std::process::Command::new("git")
        .current_dir(&cwd)
        .args(&args)
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    for line in text.lines() {
        // There is a leading %x1f, so split gives ["", H, P, h, s, an, ad, D].
        let mut it = line.split('\u{1f}');
        let _lead = it.next();
        let id = it.next().unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }
        let parents = it
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        commits.push(DagCommit {
            id,
            parents,
            short: it.next().unwrap_or("").to_string(),
            subject: it.next().unwrap_or("").to_string(),
            author: it.next().unwrap_or("").to_string(),
            date: it.next().unwrap_or("").to_string(),
            refs: it.next().unwrap_or("").to_string(),
        });
    }
    commits
}

/// An active lane (a vertical line waiting for an ancestor not yet drawn). `target` = the commit that comes to that lane next.
#[cfg(feature = "git")]
#[derive(Clone)]
struct Lane {
    target: String,
    color: ratatui::style::Color,
}

/// Assigns the DAG to lanes and returns the `GraphRow` sequence connected with angular box-drawing (a pure function = testable).
/// One lane = **2 columns** (node/`│` plus the gap on the right). The gap becomes `─` when connecting (`├─┐`/`├─┘`).
/// Rules: a commit sits on the leftmost matching lane (`my_lane`). The first parent continues its own lane; the second parent onward
/// opens a new lane **to the right** (fork = `├─┐` **below** the commit row). When multiple lanes reach the same commit, fold them into the leftmost
/// (merge = `├─┘` **above** the commit row). Lanes are not reused (parallel lanes waiting for the same ancestor merge at that ancestor),
/// so forks/merges are always to the right of my_lane = a simple implementation.
#[cfg(feature = "git")]
fn lay_out_lanes(
    commits: &[DagCommit],
    base: Option<&str>,
    wt: Option<(String, String)>,
) -> Vec<GraphRow> {
    use ratatui::style::Color;

    // Uncommitted working-tree changes (a pseudo "Uncommitted changes" row).
    // **Splice it into the DAG as a virtual commit whose parent is HEAD's commit**, so lane
    // assignment automatically places `●` at the head of HEAD's lane and connects it to HEAD with
    // `│` (the diff target matches HEAD).
    // When HEAD is out of range / unborn, it degrades to a parentless, standalone node (leftmost).
    const WT_ID: &str = "\u{1}WORKTREE\u{1}";
    let work: Vec<DagCommit> = if let Some((subject, date)) = wt.as_ref() {
        let head = head_id_of(commits);
        let mut v = Vec::with_capacity(commits.len() + 1);
        v.push(DagCommit {
            id: WT_ID.to_string(),
            parents: head.into_iter().collect(),
            short: String::new(),
            subject: subject.clone(),
            author: String::new(),
            date: date.clone(),
            refs: String::new(),
        });
        v.extend(commits.iter().cloned());
        v
    } else {
        commits.to_vec()
    };
    let commits = &work[..];

    // A cyclic palette for non-base lanes. The base (lane0) has a fixed color (BASE, below).
    const PALETTE: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Magenta,
        Color::Blue,
        Color::LightRed,
        Color::LightYellow,
    ];
    const BASE: Color = Color::White; // lane0 = base (the left trunk). A fixed color close to the theme's foreground.

    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows: Vec<GraphRow> = Vec::new();

    // Phase 2: when a base is given, reserve lane0 for the base branch's tip. From then on, lane0
    // keeps following the base's first-parent (= a straight line on the left). Every other
    // commit/new tip/branch gets pushed out to lane1 and beyond (right) (`base_floor`).
    let base_floor = if let Some(tip) = base {
        lanes.push(Some(Lane {
            target: tip.to_string(),
            color: BASE,
        }));
        1
    } else {
        0
    };

    // Color for a new lane: lane0 is the fixed BASE, everything else cycles through the palette.
    let pick_color = |idx: usize, next: &mut usize| -> Color {
        if idx == 0 {
            BASE
        } else {
            let c = PALETTE[*next % PALETTE.len()];
            *next += 1;
            c
        }
    };
    // The first free lane number at or after `start` (append at the end if there is none). A
    // branch always comes out at my_lane+1 or beyond = to the right.
    let free_from = |lanes: &mut Vec<Option<Lane>>, start: usize| -> usize {
        if let Some(i) = (start..lanes.len()).find(|&i| lanes[i].is_none()) {
            i
        } else {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    // Assemble the commit row (2 columns per lane). my_lane = the node, other active lanes = │,
    // gaps = blank.
    let commit_cells = |lanes: &[Option<Lane>], my_lane: usize, node: char, my_color: Color| {
        let n = lanes.len();
        let mut glyph = vec![' '; n.saturating_mul(2).saturating_sub(1).max(1)];
        let mut color = vec![Color::Reset; glyph.len()];
        for (i, l) in lanes.iter().enumerate() {
            if i == my_lane {
                glyph[i * 2] = node;
                color[i * 2] = my_color;
            } else if let Some(l) = l {
                glyph[i * 2] = '│';
                color[i * 2] = l.color;
            }
        }
        cells_from(glyph, color)
    };

    for c in commits {
        // 1) The lane this commit rides on (the leftmost match). A new tip if there is none.
        let hits: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match l {
                Some(l) if l.target == c.id => Some(i),
                _ => None,
            })
            .collect();
        let my_lane = if let Some(&first) = hits.first() {
            first
        } else {
            // A new tip. When a base is given, avoid lane0 and go to lane1 or beyond (so it
            // doesn't intrude on the base's straight line).
            let idx = free_from(&mut lanes, base_floor);
            let col = pick_color(idx, &mut next_color);
            lanes[idx] = Some(Lane {
                target: c.id.clone(),
                color: col,
            });
            idx
        };
        let my_color = lanes[my_lane].as_ref().map(|l| l.color).unwrap_or(BASE);

        // 2) A merge (multiple matching lanes) inserts a connector row **above** the commit row
        // and folds away the extra lanes.
        let merged: Vec<(usize, Color)> = hits
            .iter()
            .skip(1)
            .filter_map(|&i| lanes[i].as_ref().map(|l| (i, l.color)))
            .collect();
        if !merged.is_empty() {
            let conn = build_connector(&lanes, my_lane, my_color, &merged, false);
            rows.push(connector_row(conn));
            for &(i, _) in &merged {
                lanes[i] = None;
            }
        }

        // 3) The commit row.
        let node = if c.parents.len() >= 2 { '◆' } else { '●' };
        rows.push(GraphRow {
            graph: commit_cells(&lanes, my_lane, node, my_color),
            commit: Some(c.id.clone()),
            short: c.short.clone(),
            subject: c.subject.clone(),
            author: c.author.clone(),
            date: c.date.clone(),
            refs: c.refs.clone(),
            worktree: false,
        });

        // 4) Assign parents. 1st parent = continues in its own lane, 2nd parent onward = a new
        // lane to the right (= a branch).
        let mut forked: Vec<(usize, Color)> = Vec::new();
        if c.parents.is_empty() {
            lanes[my_lane] = None; // root: the lane ends here.
        } else {
            if let Some(l) = lanes[my_lane].as_mut() {
                l.target = c.parents[0].clone();
            }
            for p in c.parents.iter().skip(1) {
                let idx = free_from(&mut lanes, my_lane + 1);
                let col = pick_color(idx, &mut next_color);
                lanes[idx] = Some(Lane {
                    target: p.clone(),
                    color: col,
                });
                forked.push((idx, col));
            }
        }

        // 5) A branch inserts a connector row **below** the commit row.
        if !forked.is_empty() {
            let conn = build_connector(&lanes, my_lane, my_color, &forked, true);
            rows.push(connector_row(conn));
        }

        // 6) Trim trailing empty lanes.
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
    }

    // Turn the spliced-in virtual WT row back into the original "Uncommitted changes" pseudo-row:
    // clear commit, set worktree=true, and repaint the node (●) yellow-bold (keeping the lane position).
    if wt.is_some() {
        use ratatui::style::{Modifier, Style};
        if let Some(r) = rows.iter_mut().find(|r| r.commit.as_deref() == Some(WT_ID)) {
            r.commit = None;
            r.worktree = true;
            r.short = String::new();
            let node = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            for cell in r.graph.iter_mut() {
                if cell.0 == "●" || cell.0 == "◆" {
                    cell.1 = node;
                }
            }
        }
    }

    // Line up every row's graph width to the max, so subject's left edge lines up.
    let maxw = rows.iter().map(|r| r.graph.len()).max().unwrap_or(0);
    for r in &mut rows {
        while r.graph.len() < maxw {
            r.graph
                .push((" ".to_string(), ratatui::style::Style::default()));
        }
    }
    rows
}

/// Returns the OID of the **commit that HEAD points to** within the DAG (the row whose `%D` ref names include "HEAD").
/// Catches both HEAD -> branch and detached ("HEAD"). Returns None when out of range / unborn.
#[cfg(feature = "git")]
fn head_id_of(commits: &[DagCommit]) -> Option<String> {
    commits
        .iter()
        .find(|c| {
            c.refs
                .split(',')
                .map(|r| r.trim())
                .any(|r| r == "HEAD" || r.starts_with("HEAD ->"))
        })
        .map(|c| c.id.clone())
}

/// (glyph, color) arrays -> `Vec<(String, Style)>` with trailing blanks trimmed.
#[cfg(feature = "git")]
fn cells_from(
    mut glyph: Vec<char>,
    mut color: Vec<ratatui::style::Color>,
) -> Vec<(String, ratatui::style::Style)> {
    use ratatui::style::Style;
    while glyph.len() > 1 && *glyph.last().unwrap() == ' ' {
        glyph.pop();
        color.pop();
    }
    glyph
        .into_iter()
        .zip(color)
        .map(|(g, c)| (g.to_string(), Style::new().fg(c)))
        .collect()
}

/// A connector-only `GraphRow` (no commit).
#[cfg(feature = "git")]
fn connector_row(graph: Vec<(String, ratatui::style::Style)>) -> GraphRow {
    GraphRow {
        graph,
        commit: None,
        short: String::new(),
        subject: String::new(),
        author: String::new(),
        date: String::new(),
        refs: String::new(),
        worktree: false,
    }
}

// Constants and table for deciding the connector character from "connection-direction bits
// (up/down/left/right)".
#[cfg(feature = "git")]
const DIR_U: u8 = 1;
#[cfg(feature = "git")]
const DIR_D: u8 = 2;
#[cfg(feature = "git")]
const DIR_L: u8 = 4;
#[cfg(feature = "git")]
const DIR_R: u8 = 8;

/// A set of connection directions -> a single angular box-drawing character. Also renders crossings of multi-fork/merge (┼ ┬ ┴ ├ ┤) correctly.
#[cfg(feature = "git")]
fn glyph_for(mask: u8) -> char {
    match mask {
        m if m == DIR_U | DIR_D => '│',
        m if m == DIR_L | DIR_R => '─',
        m if m == DIR_U | DIR_D | DIR_L | DIR_R => '┼',
        m if m == DIR_U | DIR_D | DIR_R => '├',
        m if m == DIR_U | DIR_D | DIR_L => '┤',
        m if m == DIR_U | DIR_L | DIR_R => '┴',
        m if m == DIR_D | DIR_L | DIR_R => '┬',
        m if m == DIR_D | DIR_L => '┐',
        m if m == DIR_U | DIR_L => '┘',
        m if m == DIR_D | DIR_R => '┌',
        m if m == DIR_U | DIR_R => '└',
        m if m == DIR_U => '│',
        m if m == DIR_D => '│',
        m if m == DIR_L || m == DIR_R => '─',
        _ => ' ',
    }
}

/// Builds a connector row with angular box-drawing (2 columns/lane). `endpoints` = the partner lanes extending to the right (index, color).
/// `is_fork` = true: branches to the right **below** the commit (corner `┐` = down + left). false: merges from the right **above** the commit (corner `┘` = up + left).
/// Each cell accumulates connection-direction bits and then `glyph_for` maps it to a character, so crossings where a horizontal pierces another lane
/// (a passing vertical = `┼` / mid-merge = `┴` / mid-fork = `┬`) do not break. Colors: corner = partner color, horizontal = that branch's color, my_lane = own color.
#[cfg(feature = "git")]
fn build_connector(
    active: &[Option<Lane>],
    my_lane: usize,
    my_color: ratatui::style::Color,
    endpoints: &[(usize, ratatui::style::Color)],
    is_fork: bool,
) -> Vec<(String, ratatui::style::Style)> {
    use ratatui::style::Color;
    let max_e = endpoints.iter().map(|&(i, _)| i).max().unwrap_or(my_lane);
    let lanes_n = active.len().max(max_e + 1);
    let width = lanes_n.saturating_mul(2).saturating_sub(1).max(1);
    let mut conn = vec![0u8; width];
    let mut color = vec![Color::Reset; width];
    let endcols: std::collections::HashSet<usize> = endpoints.iter().map(|&(i, _)| i).collect();

    // 1) A vertical lane just passing through (an active lane that is neither my_lane nor an
    // endpoint) = up+down.
    for (i, l) in active.iter().enumerate() {
        if i == my_lane || endcols.contains(&i) {
            continue;
        }
        if let Some(l) = l {
            conn[i * 2] |= DIR_U | DIR_D;
            color[i * 2] = l.color;
        }
    }
    // 2) my_lane is up+down (continuing) + right.
    conn[my_lane * 2] |= DIR_U | DIR_D | DIR_R;
    color[my_lane * 2] = my_color;
    // 3) Lay the horizontal all at once from my_lane's right neighbor to the farthest endpoint
    // (piercing through the lanes in between).
    for slot in conn[(my_lane * 2 + 1)..(max_e * 2)].iter_mut() {
        *slot |= DIR_L | DIR_R;
    }
    // 4) The endpoint's corner bits. Fork = down+left / merge = up+left. A nearer endpoint gets
    // pierced by the horizontal, so it becomes a T (┬/┴).
    let base = if is_fork {
        DIR_D | DIR_L
    } else {
        DIR_U | DIR_L
    };
    let mut sorted: Vec<(usize, Color)> = endpoints.to_vec();
    sorted.sort_by_key(|&(i, _)| i);
    let mut cursor = my_lane * 2; // Paint each horizontal cell with the color of the nearest endpoint to its right.
    for (e, c) in sorted {
        for slot in color[(cursor + 1)..(e * 2)].iter_mut() {
            if *slot == Color::Reset {
                *slot = c;
            }
        }
        conn[e * 2] |= base;
        color[e * 2] = c;
        cursor = e * 2;
    }

    let glyph: Vec<char> = conn.iter().map(|&m| glyph_for(m)).collect();
    cells_from(glyph, color)
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn graph_with_base(
    _root: &Path,
    _base: Option<&str>,
    _lang: crate::i18n::Lang,
    _refs: Option<&[String]>,
) -> Vec<GraphRow> {
    Vec::new()
}

/// Returns the heading (`subject`) and the count breakdown (`date` field) of uncommitted working-tree changes.
/// Returns None when there are no changes. `lay_out_lanes` places the `●` on the graph at the head of the HEAD lane.
#[cfg(feature = "git")]
fn worktree_payload(root: &Path, lang: crate::i18n::Lang) -> Option<(String, String)> {
    let entries = changed_files(root);
    if entries.is_empty() {
        return None;
    }
    let (mut staged, mut unstaged, mut untracked) = (0usize, 0usize, 0usize);
    for e in &entries {
        match e.status {
            FileStatus::Untracked => untracked += 1,
            _ if e.staged => staged += 1,
            _ => unstaged += 1,
        }
    }
    let subject = crate::i18n::tr(lang, crate::i18n::Msg::UncommittedChanges).to_string();
    let date = match lang {
        crate::i18n::Lang::En => {
            format!("{staged} staged · {unstaged} unstaged · {untracked} untracked")
        }
        crate::i18n::Lang::Jp => {
            format!("{staged} ステージ済 · {unstaged} 未ステージ · {untracked} 未追跡")
        }
    };
    Some((subject, date))
}

/// Returns the uncommitted working-tree diff (HEAD -> index+workdir, including untracked content) separated by file-boundary headers.
/// For the full-screen diff when pressing `Enter` on the graph's "Uncommitted changes" row. Empty when outside a repo / on failure / when the feature is disabled.
#[cfg(feature = "git")]
pub fn worktree_diff(root: &Path) -> Vec<DiffLine> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let head_tree = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .and_then(|c| c.tree().ok());
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let diff = match repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    collect_diff_lines(&diff, true)
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn worktree_diff(_root: &Path) -> Vec<DiffLine> {
    Vec::new()
}

/// Returns the diff from `base`'s merge-base with HEAD, through the index, to the working tree —
/// committed **and** uncommitted changes together — equivalent to
/// `git diff $(git merge-base <base> HEAD)`. This is "everything this worktree has piled up since
/// branching off `base`", which is what the worktree list's `d` wants to show: an AI agent working
/// inside a worktree very often commits along the way, so an uncommitted-only diff
/// (`worktree_diff`) would go empty the moment it commits, hiding everything it actually did.
/// Diffing from the merge-base shows committed and uncommitted work together, correctly, across
/// any mix of the two.
///
/// `root` is the **target worktree's own checkout path** (not necessarily the caller's current
/// root — this is meant to inspect a *different* linked worktree picked from the list). Opening the
/// repo via `Repository::discover(root)` from inside a specific worktree gives a `Repository`
/// bound to that worktree's own HEAD (libgit2 resolves the per-worktree `HEAD`/index through its
/// private `.git/worktrees/<name>` git-dir), so `repo.head()` below is that worktree's HEAD, not
/// whichever worktree happened to open the repo first — same reasoning `worktree_diff` already
/// relies on. Local branches (`base`) are shared repo-wide via the common git-dir, so `find_branch`
/// resolves the same commit regardless of which worktree's path was used to discover the repo.
///
/// Returns an empty Vec when not a repo / `base` does not exist / HEAD is unborn / there is no
/// merge-base / on any other failure or when the feature is disabled — same "quietly render blank"
/// contract as every other diff function here, so the caller can fall back to `worktree_diff`.
#[cfg(feature = "git")]
pub fn diff_since(root: &Path, base: &str) -> Vec<DiffLine> {
    if !external_git_enabled() {
        return Vec::new();
    }
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let Ok(base_branch) = repo.find_branch(base, git2::BranchType::Local) else {
        return Vec::new();
    };
    let Some(base_oid) = base_branch.get().peel_to_commit().ok().map(|c| c.id()) else {
        return Vec::new();
    };
    let Some(head_oid) = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id())
    else {
        return Vec::new(); // unborn HEAD
    };
    let Ok(merge_base_oid) = repo.merge_base(base_oid, head_oid) else {
        return Vec::new(); // no common ancestor (unrelated histories)
    };
    let Some(base_tree) = repo
        .find_commit(merge_base_oid)
        .ok()
        .and_then(|c| c.tree().ok())
    else {
        return Vec::new();
    };
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let diff = match repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    collect_diff_lines(&diff, true)
}

#[cfg(not(feature = "git"))]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn diff_since(_root: &Path, _base: &str) -> Vec<DiffLine> {
    Vec::new()
}

/// The commit time (Unix epoch seconds) of `git merge-base <base> HEAD`, i.e. the point where
/// `base` and this worktree's HEAD last shared history. Used by `App::worktree_diff_base`
/// (`src/app/git_view.rs`) to rank several candidate base branches for the worktree list's `d`
/// diff: the candidate whose merge-base is *newest* is the branch this worktree most recently
/// diverged from, which is the base that actually explains "everything since base" without
/// dragging in an unrelated branch's own unrelated history.
///
/// `root` must be the **target worktree's own checkout path**, not necessarily the caller's
/// current root — same reasoning `diff_since` above already documents: `Repository::discover
/// (root)` resolves a `Repository` bound to *that* worktree's private per-worktree `HEAD` (via its
/// own `.git/worktrees/<name>` git-dir), so `repo.head()` below returns that worktree's HEAD, not
/// whichever worktree happened to open the repo. `base` is a local branch name, shared repo-wide
/// through the common git-dir, so it resolves to the same commit regardless of which worktree's
/// path was used to discover the repo.
///
/// Returns `None` when not a repo / `base` does not exist / HEAD is unborn / there is no common
/// ancestor / the merge-base commit can't be read / the feature is disabled — same "quietly return
/// nothing, caller treats this candidate as unusable" contract as `branch_tip`/`diff_since`.
#[cfg(feature = "git")]
pub fn merge_base_time(root: &Path, base: &str) -> Option<i64> {
    if !external_git_enabled() {
        return None;
    }
    let repo = git2::Repository::discover(root).ok()?;
    let base_oid = repo
        .find_branch(base, git2::BranchType::Local)
        .ok()?
        .get()
        .peel_to_commit()
        .ok()?
        .id();
    let head_oid = repo.head().ok()?.peel_to_commit().ok()?.id();
    let merge_base_oid = repo.merge_base(base_oid, head_oid).ok()?;
    repo.find_commit(merge_base_oid)
        .ok()
        .map(|c| c.time().seconds())
}

#[cfg(not(feature = "git"))]
pub fn merge_base_time(_root: &Path, _base: &str) -> Option<i64> {
    None
}

#[cfg(not(feature = "git"))]
pub fn stage(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn unstage(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn stage_all(_root: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn unstage_all(_root: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn discard(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn commit(_root: &Path, _message: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn checkout(_root: &Path, _name: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn create_branch(_root: &Path, _name: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn delete_branch(_root: &Path, _name: &str, _force: bool) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

#[cfg(not(feature = "git"))]
pub fn worktree_add(
    _root: &Path,
    _path: &Path,
    _branch: &str,
    _create_branch: bool,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature disabled"))
}

/// Applies the status to a file and its ancestor directories (up to workdir). Directories keep the higher priority.
#[cfg(feature = "git")]
fn rollup(map: &mut HashMap<PathBuf, FileStatus>, workdir: &Path, abs: &Path, st: FileStatus) {
    map.entry(abs.to_path_buf())
        .and_modify(|e| {
            if st.rank() > e.rank() {
                *e = st;
            }
        })
        .or_insert(st);
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        if !dir.starts_with(workdir) {
            break;
        }
        map.entry(dir.to_path_buf())
            .and_modify(|e| {
                if st.rank() > e.rank() {
                    *e = st;
                }
            })
            .or_insert(st);
        if dir == workdir {
            break;
        }
        cur = dir.parent();
    }
}

/// Maps git2 Status bits to a FileStatus. After `changed_files` was switched to CLI delegation it became unused in the main code
/// and is now only a test reference (`classify_porcelain` is the runtime implementation that aligns the priority order).
/// Compiled only for the git-feature test build, since that is its sole caller (`classify_maps_status_bits`).
#[cfg(all(test, feature = "git"))]
fn classify(s: git2::Status) -> FileStatus {
    if s.is_conflicted() {
        FileStatus::Conflicted
    } else if s.is_wt_deleted() || s.is_index_deleted() {
        FileStatus::Deleted
    } else if s.is_index_new() {
        FileStatus::Added
    } else if s.is_wt_new() {
        FileStatus::Untracked
    } else if s.is_wt_renamed() || s.is_index_renamed() {
        FileStatus::Renamed
    } else if s.is_wt_modified() || s.is_index_modified() {
        FileStatus::Modified
    } else if s.is_wt_typechange() || s.is_index_typechange() {
        FileStatus::TypeChange
    } else {
        FileStatus::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `diff_contents` (the diff engine for the follow baseline diff) emits only the changed hunks
    /// with correct line numbers (not the whole file); text never contains a newline; identical
    /// content produces an empty result.
    #[test]
    fn diff_contents_emits_changed_hunks_with_line_numbers() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nCHANGED\nd\ne\n";
        let lines = diff_contents(old, new);
        let removed: Vec<&DiffLine> = lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Removed)
            .collect();
        let added: Vec<&DiffLine> = lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(removed.len(), 1, "変更行1本が Removed");
        assert_eq!(added.len(), 1, "変更行1本が Added");
        assert_eq!(removed[0].text, "c");
        assert_eq!(removed[0].old_no, Some(3), "旧側の行番号");
        assert_eq!(added[0].text, "CHANGED");
        assert_eq!(added[0].new_no, Some(3), "新側の行番号");
        // A context line is Context, and text never contains a newline.
        assert!(lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Context && l.text == "a" && l.old_no == Some(1)));
        assert!(lines.iter().all(|l| !l.text.contains('\n')));
        // Identical content produces an empty result (no changes).
        assert!(diff_contents(new, new).is_empty());
    }

    /// Background reads (statuses/ignored) **never write back** the index (--no-optional-locks).
    /// Confirm that reading status while the stat cache is stale (content identical, only mtime
    /// updated) does not change .git/index. If this breaks, a `git pull` running while konoma is
    /// scanning status fails with "index.lock: File exists" (user report 2026-07-07).
    #[test]
    fn background_reads_never_write_the_index() {
        let dir = std::env::temp_dir().join("konoma_no_optional_locks_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {:?}", out);
        };
        run(&["init", "-q", "."]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), b"same content").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        // Update mtime while keeping the content identical → the index's stat cache goes stale.
        // A plain, flag-less `git status` would write back the index here (= take index.lock).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("a.txt"), b"same content").unwrap();

        let index = dir.join(".git/index");
        let before = std::fs::metadata(&index).unwrap().modified().unwrap();
        let st = statuses(&dir);
        let ig = ignored(&dir);
        // git2's gutter diff (diff_tree_to_workdir_with_index) doesn't set update_index either, so
        // it doesn't write back the index. This is the assumption that keeps background git reads
        // lock-free, which in turn is why we can afford to not swallow `.git/*.lock` events (so the
        // fs-watch self-loop never happens).
        let _diff = file_diff(&dir, &dir.join("a.txt"));
        let after = std::fs::metadata(&index).unwrap().modified().unwrap();
        assert_eq!(before, after, "読み取りで index を書き戻さない");
        assert!(
            st.is_empty(),
            "内容同一なので clean 判定は正しく出る: {st:?}"
        );
        assert!(ig.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `[external] git = false` (via `set_external_git_enabled(false)`): every read returns the same
    /// empty/None value as a `--no-default-features` build, and every write returns `Err`, **even
    /// though the directory is a real repo with real history** — proving the gate actually short-
    /// circuits before touching git2/the `git` CLI, rather than the assertions holding "by accident"
    /// because the repo happens to be empty. Runs entirely on this test's own thread (thread-local),
    /// so it does not affect any other test running concurrently — see the `EXTERNAL_GIT_ENABLED` doc.
    #[cfg(feature = "git")]
    #[test]
    fn external_git_disabled_returns_empty_for_a_real_repo() {
        let dir = std::env::temp_dir().join("konoma_git_external_disabled_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let file = dir.join("a.txt");
        std::fs::write(&file, b"one\n").unwrap();
        // Baseline sanity check while still enabled (default): the repo really is a repo.
        assert!(external_git_enabled(), "default is enabled");
        assert!(workdir(&dir).is_some(), "sanity: this is a real repo");
        stage(&dir, &file).unwrap();
        commit(&dir, "init").unwrap();
        assert!(branch(&dir).is_some(), "sanity: has a branch after commit");
        std::fs::write(&file, b"two\n").unwrap();

        set_external_git_enabled(false);
        assert!(statuses(&dir).is_empty(), "statuses");
        assert!(ignored(&dir).is_empty(), "ignored");
        assert!(workdir(&dir).is_none(), "workdir");
        assert!(branch(&dir).is_none(), "branch");
        assert!(branches(&dir).is_empty(), "branches");
        assert!(branches_by_recency(&dir).is_empty(), "branches_by_recency");
        assert!(worktrees(&dir).is_empty(), "worktrees");
        assert!(file_diff(&dir, &file).is_empty(), "file_diff");
        assert!(changed_files(&dir).is_empty(), "changed_files");
        assert!(log(&dir, 10).is_empty(), "log");
        assert!(worktree_diff(&dir).is_empty(), "worktree_diff");
        assert!(head_commit_id(&dir).is_none(), "head_commit_id");
        // Mutations: all route through the shared `run_git` gate.
        assert!(stage(&dir, &file).is_err(), "stage");
        assert!(unstage(&dir, &file).is_err(), "unstage");
        assert!(stage_all(&dir).is_err(), "stage_all");
        assert!(unstage_all(&dir).is_err(), "unstage_all");
        assert!(discard(&dir, &file).is_err(), "discard");
        assert!(commit(&dir, "nope").is_err(), "commit");
        assert!(checkout(&dir, "nope").is_err(), "checkout");
        assert!(create_branch(&dir, "nope").is_err(), "create_branch");
        assert!(delete_branch(&dir, "nope", false).is_err(), "delete_branch");
        assert!(
            worktree_add(&dir, &dir.join("nope-wt"), "nope", true).is_err(),
            "worktree_add"
        );
        assert!(
            graph_with_base(&dir, None, crate::i18n::Lang::En, None).is_empty(),
            "graph_with_base"
        );

        // Commit id/branch name resolved *before* disabling still work as lookups, but with the gate
        // off they must fail too.
        assert!(
            branch_tip(&dir, "main").is_none() && branch_tip(&dir, "master").is_none(),
            "branch_tip"
        );

        // Re-enable and confirm the gate is not "stuck off": this is the same thread that just
        // disabled it, so re-enabling must restore normal behavior immediately.
        set_external_git_enabled(true);
        assert!(
            !statuses(&dir).is_empty(),
            "statuses work again once re-enabled"
        );
        assert!(branch(&dir).is_some(), "branch works again once re-enabled");
        assert!(
            !worktrees(&dir).is_empty(),
            "worktrees work again once re-enabled"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `git worktree list --porcelain -z` parsing: main worktree first (`is_main`), the current
    /// worktree flagged (`is_current`), a linked worktree's branch/short-hash read correctly, a
    /// detached worktree has `branch: None`, and a locked worktree carries its reason. Verified
    /// against a real sandbox (not synthetic bytes) so the record shape matches actual git output.
    #[cfg(feature = "git")]
    #[test]
    fn worktrees_parses_main_linked_detached_and_locked_records() {
        let dir = std::env::temp_dir().join("konoma_git_worktrees_parse_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();
        let main_root = dir.canonicalize().unwrap();

        // `git worktree add` creates the target directory itself, so pick paths that don't exist yet.
        let linked = std::env::temp_dir().join("konoma_git_worktrees_parse_linked");
        let detached = std::env::temp_dir().join("konoma_git_worktrees_parse_detached");
        let _ = std::fs::remove_dir_all(&linked);
        let _ = std::fs::remove_dir_all(&detached);
        let sh = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&main_root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        sh(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "konoma-wt-feature",
            linked.to_str().unwrap(),
        ]);
        sh(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
        ]);
        sh(&[
            "worktree",
            "lock",
            linked.to_str().unwrap(),
            "--reason",
            "editing",
        ]);

        let list = worktrees(&main_root);
        assert!(
            list.len() >= 3,
            "main + linked + detached の3件以上: {list:?}"
        );
        assert!(list[0].is_main, "先頭レコードは必ずメインワークツリー");
        assert!(
            list[0].is_current,
            "root=main_root なので main が is_current"
        );
        assert!(
            list[1..].iter().all(|w| !w.is_main),
            "2件目以降は is_main=false"
        );

        let linked_abs = linked.canonicalize().unwrap();
        let l = list
            .iter()
            .find(|w| w.path == linked_abs)
            .expect("linked worktree が一覧に無い");
        assert_eq!(l.branch.as_deref(), Some("konoma-wt-feature"));
        assert!(!l.is_current, "linked は現在地ではない");
        assert!(!l.is_main);
        assert!(!l.is_bare);
        assert_eq!(
            l.locked.as_deref(),
            Some("editing"),
            "lock 理由が読める: {l:?}"
        );
        assert!(!l.prunable);
        assert!(
            l.head.as_deref().map(|h| h.len()).unwrap_or(0) <= 7,
            "短縮ハッシュは7文字以内: {:?}",
            l.head
        );

        let detached_abs = detached.canonicalize().unwrap();
        let d = list
            .iter()
            .find(|w| w.path == detached_abs)
            .expect("detached worktree が一覧に無い");
        assert_eq!(d.branch, None, "detached は branch=None");
        assert!(d.locked.is_none());
        assert!(!d.prunable);

        std::fs::remove_dir_all(&detached).ok();
        // Undo the lock before removing (a locked worktree resists a plain rm/prune, though a bare
        // `remove_dir_all` on the checkout itself is unaffected — this just mirrors real usage).
        let _ = std::process::Command::new("git")
            .current_dir(&main_root)
            .args(["worktree", "unlock", linked.to_str().unwrap()])
            .output();
        std::fs::remove_dir_all(&linked).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `worktree_add` against a branch another worktree already has checked out: the error message
    /// leads with git's own `fatal:` line, not the command that was run. This matters because the
    /// flash footer renders the message as one line clipped at the terminal width, and `worktree
    /// add` writes a progress line (`Preparing worktree (checking out '<branch>')`) to stderr
    /// *before* the `fatal:` line — so a naive "command: stderr.trim()" message buries the actual
    /// reason behind the (often long) command/path, past the visible width, and the user never
    /// sees why it failed. Before the fix (`run_git` prefixing `"git {args}: {stderr}"`), this test
    /// fails: the message starts with `"git worktree add "`, not `"fatal:"`.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_add_error_leads_with_gits_fatal_line_not_the_command() {
        let dir = unique_tmp("konoma_git_worktree_add_err_msg");
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();
        let root = dir.canonicalize().unwrap();

        // A first worktree already has branch "taken" checked out.
        let first = unique_tmp("konoma_git_worktree_add_err_msg_first");
        worktree_add(&root, &first, "taken", true).unwrap();

        // Second attempt on the same branch: git refuses.
        let second = unique_tmp("konoma_git_worktree_add_err_msg_second");
        let err = worktree_add(&root, &second, "taken", false)
            .expect_err("同じブランチへの2本目の worktree add は失敗する");
        let msg = err.to_string();
        assert!(
            msg.starts_with("fatal:"),
            "理由(fatal:)が先頭に来る: {msg:?}"
        );
        assert!(
            !msg.contains("worktree add"),
            "実行したコマンド文字列は含まない(理由を押し出すため): {msg:?}"
        );

        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A bare main worktree (`git clone --bare` + `git worktree add`) has no `HEAD`/`branch` of its
    /// own to switch into (`is_bare=true`, `branch=None`, `head=None`), while a linked worktree
    /// added from it parses normally. A **different** linked worktree whose checkout directory is
    /// later removed out from under git is still listed, but reported `prunable`.
    #[cfg(feature = "git")]
    #[test]
    fn worktrees_bare_main_worktree_has_no_checkout_and_gone_checkout_is_prunable() {
        let dir = std::env::temp_dir().join("konoma_git_worktrees_bare_src");
        let bare = std::env::temp_dir().join("konoma_git_worktrees_bare_repo.git");
        let wt1 = std::env::temp_dir().join("konoma_git_worktrees_bare_wt1");
        let wt2 = std::env::temp_dir().join("konoma_git_worktrees_bare_wt2");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
        let _ = std::fs::remove_dir_all(&wt1);
        let _ = std::fs::remove_dir_all(&wt2);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();

        let sh = |cwd: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        sh(
            &std::env::temp_dir(),
            &[
                "clone",
                "-q",
                "--bare",
                dir.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        sh(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt1-branch",
                wt1.to_str().unwrap(),
            ],
        );
        sh(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt2-branch",
                wt2.to_str().unwrap(),
            ],
        );

        // `root` is a *linked* (non-bare) checkout, matching real usage: konoma is pointed at an
        // ordinary working tree, not at the bare repository's own directory — `workdir()` (which
        // `worktrees()` uses to discover the repo) returns None for a bare repo opened directly, by
        // design (mirrors `statuses()`'s "not a repo / a bare repo" early return).
        let list = worktrees(&wt1);
        assert_eq!(list.len(), 3, "bare main + wt1 + wt2: {list:?}");
        let main = &list[0];
        assert!(main.is_main);
        assert!(main.is_bare, "bare メインは is_bare=true");
        assert_eq!(main.branch, None, "bare メインに branch は無い");
        assert_eq!(main.head, None, "bare メインに head は無い");
        assert!(!main.prunable);
        assert!(!main.is_current, "bare メインは現在地ではない");

        let wt1_abs = wt1.canonicalize().unwrap();
        let wt2_abs = wt2.canonicalize().unwrap();
        let w1 = list.iter().find(|w| w.path == wt1_abs).unwrap();
        assert!(!w1.is_bare);
        assert_eq!(w1.branch.as_deref(), Some("wt1-branch"));
        assert!(!w1.prunable);
        assert!(w1.is_current, "root=wt1 なので wt1 が is_current");
        let w2 = list.iter().find(|w| w.path == wt2_abs).unwrap();
        assert!(!w2.prunable);
        assert!(!w2.is_current);

        // Remove wt2's checkout out from under git (without `git worktree remove`) → git reports
        // it prunable. Query again from wt1, which is untouched and still a valid discovery root.
        std::fs::remove_dir_all(&wt2).unwrap();
        let list2 = worktrees(&wt1);
        let gone = list2
            .iter()
            .find(|w| w.path == wt2_abs)
            .expect("消えた worktree もエントリとしては残る");
        assert!(gone.prunable, "実体が無ければ prunable: {gone:?}");
        let still_here = list2.iter().find(|w| w.path == wt1_abs).unwrap();
        assert!(!still_here.prunable, "無事な方は prunable にならない");

        std::fs::remove_dir_all(&wt1).ok();
        std::fs::remove_dir_all(&bare).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `worktree_origin`: `None` for the main working tree (including from a subdirectory reached
    /// via `l`), `Some("<main's directory name>")` for a linked worktree — and the same value from a
    /// subdirectory inside that linked worktree, since discovery walks up to the repo regardless of
    /// where `root` points.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_origin_is_none_for_main_and_the_repo_name_for_a_linked_worktree() {
        let dir = unique_tmp("konoma_git_worktree_origin_main");
        let linked = unique_tmp("konoma_git_worktree_origin_linked");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&linked);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();
        let main_root = dir.canonicalize().unwrap();
        let expected_origin = main_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();

        assert_eq!(
            worktree_origin(&main_root),
            None,
            "メインの作業ツリーでは None"
        );
        std::fs::create_dir_all(main_root.join("sub")).unwrap();
        assert_eq!(
            worktree_origin(&main_root.join("sub")),
            None,
            "メインのサブディレクトリでも None"
        );

        let sh = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&main_root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        sh(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "konoma-wt-origin-feature",
            linked.to_str().unwrap(),
        ]);
        let linked_abs = linked.canonicalize().unwrap();

        assert_eq!(
            worktree_origin(&linked_abs),
            Some(expected_origin.clone()),
            "リンクワークツリーではメインの repo 名"
        );
        std::fs::create_dir_all(linked_abs.join("nested")).unwrap();
        assert_eq!(
            worktree_origin(&linked_abs.join("nested")),
            Some(expected_origin),
            "リンクワークツリーのサブディレクトリでも同じ値"
        );

        std::fs::remove_dir_all(&linked).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `worktree_origin` in a **bare** layout (`git clone --bare` + `git worktree add`): there is no
    /// `.git` directory inside a bare repo, so `commondir()` is the bare repo's own directory
    /// (`<name>.git/`) — the origin name is that basename with the trailing `.git` stripped.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_origin_bare_layout_strips_the_trailing_git_suffix() {
        let dir = unique_tmp("konoma_git_worktree_origin_bare_src");
        let bare = unique_tmp("konoma_git_worktree_origin_bare_repo").with_extension("git");
        let wt = unique_tmp("konoma_git_worktree_origin_bare_wt");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
        let _ = std::fs::remove_dir_all(&wt);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();

        let sh = |cwd: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        sh(
            &std::env::temp_dir(),
            &[
                "clone",
                "-q",
                "--bare",
                dir.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        sh(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt-branch",
                wt.to_str().unwrap(),
            ],
        );

        let bare_abs = bare.canonicalize().unwrap();
        let expected_origin = bare_abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .strip_suffix(".git")
            .unwrap()
            .to_string();
        let wt_abs = wt.canonicalize().unwrap();

        assert_eq!(
            worktree_origin(&wt_abs),
            Some(expected_origin),
            "bare レイアウトでは commondir 自身の名前から `.git` を落とした名前"
        );

        std::fs::remove_dir_all(&wt).ok();
        std::fs::remove_dir_all(&bare).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole point of `diff_since` over `worktree_diff`: it must show **both** what the
    /// worktree has already committed since branching off `base` **and** what's still uncommitted
    /// on top of that — because an AI agent working in a worktree very often commits along the
    /// way, and a plain uncommitted-only diff would go blank the moment it does, hiding everything
    /// it actually produced. This test would fail if `diff_since` were implemented as a thin
    /// wrapper around `worktree_diff` (HEAD → workdir+index only): the committed file's content
    /// would then be absent from the result, since it's already at HEAD and would show as
    /// unchanged.
    #[cfg(feature = "git")]
    #[test]
    fn diff_since_includes_both_committed_and_uncommitted_changes() {
        let dir = std::env::temp_dir().join("konoma_git_diff_since_both_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("base.txt"), b"base content\n").unwrap();
        stage(&dir, &dir.join("base.txt")).unwrap();
        commit(&dir, "base commit").unwrap();
        let main_root = dir.canonicalize().unwrap();

        // A linked worktree branched off `main` (whatever the initial branch is named — read it
        // back rather than assuming "main", since git's default varies by user/global config).
        let base_name = branch(&main_root).expect("sanity: has a branch after the base commit");
        let linked = std::env::temp_dir().join("konoma_git_diff_since_both_linked");
        let _ = std::fs::remove_dir_all(&linked);
        let sh = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&main_root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        sh(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "konoma-diff-since-feature",
            linked.to_str().unwrap(),
        ]);
        let linked = linked.canonicalize().unwrap();

        // ① A commit made *inside the worktree* — the kind of thing an agent does mid-task. Named
        // so its filename/content share no substring with the uncommitted file below (a naive
        // "committed"/"uncommitted" naming would have "committed.txt" as a substring of
        // "uncommitted.txt", making the sanity check below vacuously true).
        std::fs::write(linked.join("agent_landed.txt"), b"AGENT_LANDED_MARKER\n").unwrap();
        stage(&linked, &linked.join("agent_landed.txt")).unwrap();
        commit(&linked, "agent commit inside the worktree").unwrap();

        // ② On top of that, still-pending (uncommitted) work.
        std::fs::write(linked.join("agent_pending.txt"), b"AGENT_PENDING_MARKER\n").unwrap();

        let lines = diff_since(&linked, &base_name);
        assert!(
            !lines.is_empty(),
            "base からの積み上げがあるので空のはずがない"
        );
        let joined: String = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("agent_landed.txt") && joined.contains("AGENT_LANDED_MARKER"),
            "コミット済みの変更が含まれる（worktree_diff だけの実装ならここが無い）: {joined}"
        );
        assert!(
            joined.contains("agent_pending.txt") && joined.contains("AGENT_PENDING_MARKER"),
            "未コミットの変更も含まれる: {joined}"
        );

        // Sanity check on the premise itself: a plain uncommitted-only diff must NOT contain the
        // committed file's content (it's already at HEAD, so it reads as unchanged) — proving the
        // two functions really do answer different questions, and this test isn't accidentally
        // passing for some other reason. This is exactly the assertion that would fail if
        // `diff_since` were swapped for `worktree_diff`.
        let uncommitted_only = worktree_diff(&linked);
        let uncommitted_only_joined: String = uncommitted_only
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !uncommitted_only_joined.contains("AGENT_LANDED_MARKER")
                && !uncommitted_only_joined.contains("agent_landed.txt"),
            "前提の確認: worktree_diff は未コミットのみで、コミット済みの内容は含まれないはず: {uncommitted_only_joined}"
        );

        std::fs::remove_dir_all(&linked).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `base` that doesn't exist in the repo → empty Vec, so the caller's fallback path
    /// (`worktree_diff`) actually runs instead of panicking or silently returning garbage.
    #[cfg(feature = "git")]
    #[test]
    fn diff_since_returns_empty_for_a_nonexistent_base() {
        let dir = std::env::temp_dir().join("konoma_git_diff_since_missing_base_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        stage(&dir, &dir.join("a.txt")).unwrap();
        commit(&dir, "init").unwrap();
        std::fs::write(dir.join("a.txt"), b"two\n").unwrap(); // an uncommitted change too

        assert!(
            diff_since(&dir, "this-branch-does-not-exist").is_empty(),
            "存在しない base は空 Vec（フォールバック経路が動く）"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn lay_out_lanes_draws_angular_fork_and_merge() {
        // M (merge B,F) → B (→R) / F (→R) → R (root). A branch = ├─┐ below the commit, a merge =
        // ├─┘ above the commit.
        let dc = |id: &str, parents: &[&str]| DagCommit {
            id: id.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            short: id.into(),
            subject: format!("{id} subj"),
            author: "a".into(),
            date: "d".into(),
            refs: String::new(),
        };
        let commits = vec![
            dc("M", &["B", "F"]),
            dc("B", &["R"]),
            dc("F", &["R"]),
            dc("R", &[]),
        ];
        let rows = lay_out_lanes(&commits, None, None);
        let joined: Vec<String> = rows
            .iter()
            .map(|r| {
                r.graph
                    .iter()
                    .map(|(s, _)| s.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert_eq!(
            joined,
            vec!["◆", "├─┐", "● │", "│ ●", "├─┘", "●"],
            "角ばったグラフが期待と不一致: {joined:?}"
        );
        // No diagonal lines at all (consistent with an angular TUI).
        let all: String = rows
            .iter()
            .flat_map(|r| r.graph.iter().map(|(s, _)| s.clone()))
            .collect();
        assert!(
            !all.contains('/') && !all.contains('\\'),
            "斜め線が残っている: {all}"
        );
        // Nodes: merge = ◆ (1) · normal = ● (3). There are 4 commit rows.
        assert_eq!(all.matches('◆').count(), 1, "マージノード ◆ は1個");
        assert_eq!(all.matches('●').count(), 3, "通常ノード ● は3個");
        assert_eq!(
            rows.iter().filter(|r| r.commit.is_some()).count(),
            4,
            "コミット行は4"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn lay_out_lanes_multi_converge_uses_tee() {
        // Three tips merge into the same root R → the middle lane is ┴ (up+left+right), the
        // farthest is ┘. Must not become `├───┘`.
        let dc = |id: &str, parents: &[&str]| DagCommit {
            id: id.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            short: id.into(),
            subject: id.into(),
            author: "a".into(),
            date: "d".into(),
            refs: String::new(),
        };
        let commits = vec![
            dc("T1", &["R"]),
            dc("T2", &["R"]),
            dc("T3", &["R"]),
            dc("R", &[]),
        ];
        let rows = lay_out_lanes(&commits, None, None);
        let joined: Vec<String> = rows
            .iter()
            .map(|r| {
                r.graph
                    .iter()
                    .map(|(s, _)| s.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert!(
            joined.contains(&"├─┴─┘".to_string()),
            "多重合流が ┴ を使っていない(角が水平で潰れた?): {joined:?}"
        );
        assert!(
            !joined.iter().any(|r| r.contains("───")),
            "合流の途中が水平で潰れている: {joined:?}"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn lay_out_lanes_base_pins_branch_to_lane0() {
        // Two branches (A: A1→A2→R / B: B1→B2→R). With base specified, that branch's node always
        // lands on lane0 (col0).
        let dc = |id: &str, parents: &[&str]| DagCommit {
            id: id.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            short: id.into(),
            subject: id.into(),
            author: "a".into(),
            date: "d".into(),
            refs: String::new(),
        };
        let commits = vec![
            dc("A1", &["A2"]),
            dc("A2", &["R"]),
            dc("B1", &["B2"]),
            dc("B2", &["R"]),
            dc("R", &[]),
        ];
        // Whether the commit id's node sits at col0 (lane0).
        let node_at_col0 = |rows: &[GraphRow], id: &str| -> bool {
            rows.iter().any(|r| {
                r.commit.as_deref() == Some(id)
                    && matches!(r.graph.first(), Some((s, _)) if s == "●" || s == "◆")
            })
        };

        // No base: A, which appears first, gets lane0; B goes to the right.
        let none = lay_out_lanes(&commits, None, None);
        assert!(node_at_col0(&none, "A1"), "base なしでは A1 が lane0");
        assert!(
            !node_at_col0(&none, "B1"),
            "base なしでは B1 は lane0 でない"
        );

        // base=B1 (feature's tip): the B lineage goes to lane0, A gets pushed to the right.
        let based = lay_out_lanes(&commits, Some("B1"), None);
        assert!(
            node_at_col0(&based, "B1"),
            "base=B1 で B1 が lane0: {based:?}",
        );
        assert!(
            node_at_col0(&based, "B2"),
            "base=B1 で B2 も lane0(first-parent 継続)"
        );
        assert!(
            !node_at_col0(&based, "A1"),
            "base=B1 で A1 は右レーン(lane0 でない)"
        );
        // Every commit row is kept (none disappear).
        assert_eq!(
            based.iter().filter(|r| r.commit.is_some()).count(),
            5,
            "全コミットが残る(--all 相当)"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn worktree_row_sits_on_head_lane_not_always_col0() {
        // HEAD=A1 (branch A's tip). With base=B1, lane0 is the B lineage. The uncommitted row must
        // sit **on A's lane (not col0)** and connect vertically directly above HEAD (the diff
        // target matches HEAD).
        let dc = |id: &str, parents: &[&str], refs: &str| DagCommit {
            id: id.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            short: id.into(),
            subject: id.into(),
            author: "a".into(),
            date: "d".into(),
            refs: refs.into(),
        };
        let commits = vec![
            dc("A1", &["A2"], "HEAD -> feature"),
            dc("A2", &["R"], ""),
            dc("B1", &["B2"], ""),
            dc("B2", &["R"], ""),
            dc("R", &[], ""),
        ];
        let wt = Some(("Uncommitted changes".to_string(), "1 staged".to_string()));

        // base=B1: lane0 = the B lineage. The WT row is worktree=true, and col0 is not `●` (HEAD=A
        // is in the right lane).
        let rows = lay_out_lanes(&commits, Some("B1"), wt.clone());
        let wt_row = rows.iter().find(|r| r.worktree).expect("WT 行がある");
        assert!(wt_row.commit.is_none(), "WT 行は commit=None");
        assert!(
            !matches!(wt_row.graph.first(), Some((s, _)) if s == "●"),
            "base が別枝なら WT は col0 に固定されない"
        );
        assert!(
            wt_row.graph.iter().any(|(s, _)| s == "●"),
            "WT 行に ● ノードがある"
        );
        // WT's node column matches HEAD's (A1) node column (they connect vertically).
        let col_of_node = |r: &GraphRow| r.graph.iter().position(|(s, _)| s == "●" || s == "◆");
        let wt_idx = rows.iter().position(|r| r.worktree).unwrap();
        let head_idx = rows
            .iter()
            .position(|r| r.commit.as_deref() == Some("A1"))
            .unwrap();
        assert_eq!(
            col_of_node(&rows[wt_idx]),
            col_of_node(&rows[head_idx]),
            "WT のノード列が HEAD(A1)のノード列と一致"
        );

        // No base: HEAD=A is the leftmost lane0, so WT is also col0 (matches the previous display).
        let rows0 = lay_out_lanes(&commits, None, wt);
        let wt0 = rows0.iter().find(|r| r.worktree).unwrap();
        assert!(
            matches!(wt0.graph.first(), Some((s, _)) if s == "●"),
            "base なしでは WT は col0(HEAD=lane0)"
        );
    }

    #[test]
    fn marker_and_rank_are_distinct() {
        // The marker is unique per kind.
        let all = [
            FileStatus::Modified,
            FileStatus::Added,
            FileStatus::Untracked,
            FileStatus::Deleted,
            FileStatus::Renamed,
            FileStatus::TypeChange,
            FileStatus::Conflicted,
        ];
        let markers: Vec<char> = all.iter().map(|s| s.marker()).collect();
        let mut uniq = markers.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(markers.len(), uniq.len(), "マーカーが重複");
        // Rollup priority: conflicted is the strongest, untracked is the weakest.
        assert!(FileStatus::Conflicted.rank() > FileStatus::Modified.rank());
        assert!(FileStatus::Modified.rank() > FileStatus::Untracked.rank());
    }

    #[cfg(feature = "git")]
    #[test]
    fn classify_maps_status_bits() {
        use git2::Status as S;
        assert_eq!(classify(S::INDEX_RENAMED), FileStatus::Renamed);
        assert_eq!(classify(S::WT_NEW), FileStatus::Untracked);
        assert_eq!(classify(S::INDEX_NEW), FileStatus::Added);
        assert_eq!(classify(S::WT_MODIFIED), FileStatus::Modified);
        assert_eq!(classify(S::WT_DELETED), FileStatus::Deleted);
        assert_eq!(classify(S::CONFLICTED), FileStatus::Conflicted);
        assert_eq!(classify(S::WT_TYPECHANGE), FileStatus::TypeChange);
    }

    #[cfg(feature = "git")]
    #[test]
    fn untracked_file_and_dir_rollup_detected() {
        let dir = std::env::temp_dir().join("konoma_git_status_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("sub").join("foo.txt"), b"hi").unwrap();

        let map = statuses(&dir);
        let canon = dir.canonicalize().unwrap();
        // The untracked file is Untracked.
        assert_eq!(
            map.get(&canon.join("sub").join("foo.txt")),
            Some(&FileStatus::Untracked)
        );
        // It also rolls up into the parent directory.
        assert_eq!(map.get(&canon.join("sub")), Some(&FileStatus::Untracked));
        std::fs::remove_dir_all(&dir).ok();
    }

    // Pin down that a rename still shows as "Renamed on the new path" after CLI delegation
    // (porcelain -z is in new→old order).
    #[cfg(feature = "git")]
    #[test]
    fn rename_is_detected_on_new_path() {
        let dir = std::env::temp_dir().join("konoma_git_status_rename");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("old.txt"), b"content here\n").unwrap();
        stage(&dir, &dir.join("old.txt")).unwrap();
        commit(&dir, "init").unwrap();
        // Rename and stage via git mv.
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(["mv", "old.txt", "new.txt"])
            .output()
            .unwrap();

        let map = statuses(&dir);
        let canon = dir.canonicalize().unwrap();
        // Renamed shows on the new path; the old path does not appear.
        assert_eq!(
            map.get(&canon.join("new.txt")),
            Some(&FileStatus::Renamed),
            "改名は新パスに R が付くはず: {map:?}"
        );
        assert!(
            !map.contains_key(&canon.join("old.txt")),
            "旧パスはツリーに出ないので map に入らないはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    fn init_repo(dir: &Path) {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        cfg.set_str("commit.gpgsign", "false").ok();
    }

    /// A unique temp directory per call (pid + a process-global counter), same pattern as
    /// `app/tests.rs`'s helper of the same name: worktree tests create several sibling directories
    /// per case, and a fixed path would collide across parallel test runs.
    #[cfg(feature = "git")]
    fn unique_tmp(prefix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{prefix}_{}_{n}", std::process::id()))
    }

    #[cfg(feature = "git")]
    #[test]
    fn ignored_collapses_dirs_and_excludes_tracked() {
        let dir = std::env::temp_dir().join("konoma_git_ignored_set");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join(".gitignore"), b"target/\nnode_modules/\n*.log\n").unwrap();
        std::fs::create_dir_all(dir.join("target/deep")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("target/a.o"), b"x").unwrap();
        std::fs::write(dir.join("target/deep/b.o"), b"x").unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), b"x").unwrap();
        std::fs::write(dir.join("app.log"), b"x").unwrap();
        std::fs::write(dir.join("src/main.rs"), b"fn main(){}\n").unwrap();
        stage(&dir, &dir.join(".gitignore")).unwrap();
        stage(&dir, &dir.join("src/main.rs")).unwrap();
        commit(&dir, "init").unwrap();

        let set = ignored(&dir);
        let canon = dir.canonicalize().unwrap();
        // An ignored dir collapses (one dir entry, not recursed into). Equivalent to
        // recurse_ignored_dirs(false).
        assert!(
            set.contains(&canon.join("target")),
            "target/ が collapse で1件: {set:?}"
        );
        assert!(
            set.contains(&canon.join("node_modules")),
            "node_modules/ が collapse で1件: {set:?}"
        );
        assert!(
            set.contains(&canon.join("app.log")),
            "*.log の無視ファイル: {set:?}"
        );
        // Since it's collapsed, individual files inside it are not included.
        assert!(
            !set.contains(&canon.join("target/a.o")),
            "collapse 中の個別ファイルは入らない: {set:?}"
        );
        // A tracked file / .gitignore itself is not ignored.
        assert!(
            !set.contains(&canon.join("src/main.rs")),
            "追跡ファイルは無視でない"
        );
        assert!(
            !set.contains(&canon.join(".gitignore")),
            ".gitignore 自身は無視でない"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn file_diff_untracked_is_all_added() {
        let dir = std::env::temp_dir().join("konoma_git_filediff_untracked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"line1\nline2\n").unwrap();
        let diff = file_diff(&dir, &f);
        assert!(!diff.is_empty(), "未追跡ファイルの diff が空");
        assert!(
            diff.iter().all(|l| l.kind == DiffLineKind::Added),
            "未追跡は全行 Added のはず: {diff:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn file_diff_modified_has_added_and_removed() {
        let dir = std::env::temp_dir().join("konoma_git_filediff_modified");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"alpha\nbeta\n").unwrap();
        // The initial commit.
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["add", "-A"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();
        assert!(out.status.success(), "commit 失敗");
        // The change.
        std::fs::write(&f, b"alpha\ngamma\n").unwrap();
        let diff = file_diff(&dir, &f);
        assert!(
            diff.iter().any(|l| l.kind == DiffLineKind::Added),
            "Added 行が無い: {diff:?}"
        );
        assert!(
            diff.iter().any(|l| l.kind == DiffLineKind::Removed),
            "Removed 行が無い: {diff:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn changed_files_lists_staged_flag() {
        let dir = std::env::temp_dir().join("konoma_git_changed_files");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"hi\n").unwrap();
        // Before staging: untracked, staged=false.
        let before = changed_files(&dir);
        assert_eq!(before.len(), 1);
        assert!(!before[0].staged);
        // Stage via git add → staged=true.
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["add", "a.txt"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let after = changed_files(&dir);
        assert_eq!(after.len(), 1);
        assert!(after[0].staged, "add 後は staged=true のはず");
        assert!(after[0].path.ends_with("a.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn stage_commit_then_log_and_commit_diff() {
        let dir = std::env::temp_dir().join("konoma_git_stage_commit_log");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello\n").unwrap();
        // Stage via the public API → commit.
        stage(&dir, &f).unwrap();
        commit(&dir, "first commit").unwrap();
        let entries = log(&dir, 10);
        assert_eq!(entries.len(), 1, "log は1件のはず");
        assert_eq!(entries[0].summary, "first commit");
        assert_eq!(entries[0].short.len(), 7);
        // commit_diff is not empty (a new file was added).
        let cd = commit_diff(&dir, &entries[0].id);
        assert!(!cd.is_empty(), "commit_diff が空");
        assert!(
            cd.iter().any(|l| l.kind == DiffLineKind::Added),
            "Added 行が無い: {cd:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn unstage_and_discard_work() {
        let dir = std::env::temp_dir().join("konoma_git_unstage_discard");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"v1\n").unwrap();
        stage(&dir, &f).unwrap();
        commit(&dir, "init").unwrap();
        // Modify and stage → unstage removes it from the index.
        std::fs::write(&f, b"v2\n").unwrap();
        stage(&dir, &f).unwrap();
        assert!(changed_files(&dir).iter().any(|e| e.staged));
        unstage(&dir, &f).unwrap();
        assert!(
            !changed_files(&dir).iter().any(|e| e.staged),
            "unstage 後は staged が無いはず"
        );
        // discard throws away the working-tree change → clean.
        discard(&dir, &f).unwrap();
        assert!(changed_files(&dir).is_empty(), "discard 後はクリーンのはず");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    // SPEED GUARD: lay_out_lanes is a pure function that assigns lanes from a DAG (a parent-ID
    // list). Confirm it stays around O(n) even for a 1000-entry linear chain (a regression guard
    // against lane assignment turning quadratic). No real repo is needed, so a synthetic DAG is
    // passed directly. Generation is cheap, so this is not `#[ignore]`.
    #[cfg(feature = "git")]
    #[test]
    fn lay_out_lanes_linear_dag_is_bounded() {
        use std::time::{Duration, Instant};
        let n = 1000usize;
        let commits: Vec<DagCommit> = (0..n)
            .map(|i| DagCommit {
                id: format!("c{i}"),
                parents: if i + 1 < n {
                    vec![format!("c{}", i + 1)]
                } else {
                    Vec::new()
                },
                short: format!("c{i}"),
                subject: format!("subject {i}"),
                author: "a".into(),
                date: "d".into(),
                refs: String::new(),
            })
            .collect();
        let t = Instant::now();
        let rows = lay_out_lanes(&commits, None, None);
        let dt = t.elapsed();
        assert_eq!(
            rows.iter().filter(|r| r.commit.is_some()).count(),
            n,
            "全コミット行が出る"
        );
        assert!(
            dt < Duration::from_secs(2),
            "1000 コミットのレーン割当が遅すぎる(回帰?): {dt:?}"
        );
    }
}
