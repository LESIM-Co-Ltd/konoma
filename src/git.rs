// git status の取得 (FR-7)。ツリー各行に M/A/U/削除 等を色付きで出すためのデータ源。
// `git` feature(既定 on)が無い環境でも `statuses()` は空マップを返し、機能以外は動く。
//
// 仕様: リポジトリ全体の status を一度に取得し、(絶対パス → 種別) のマップにする。
// ディレクトリには配下の最も重要な変更を畳み込んで(rollup)反映する。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::style::Color;

/// Test-only counter of actual `statuses()` invocations, so a speed guard can assert the per-workdir
/// status cache reuses the result across `h`/`l` instead of re-scanning the whole worktree.
/// Only the `git`-feature `statuses` increments it and the guard is `git`-gated, so scope it to both.
#[cfg(all(test, feature = "git"))]
pub static STATUS_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
            FileStatus::Added => Color::Green, // 追加(ステージ済)= 緑
            FileStatus::Untracked => Color::LightGreen, // 未追跡 = 明るい緑(Added と区別)
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
    #[cfg(test)]
    STATUS_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    use std::os::unix::ffi::OsStrExt;
    let mut map = HashMap::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return map; // repo でない / bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // porcelain v1 -z: NUL 区切りで各レコード `XY <path>`。改名/コピー(X か Y が R/C)のときは
    // 直後の NUL フィールドが旧パス。-uall=未追跡 dir も再帰列挙(git2 の recurse_untracked_dirs 相当)、
    // --ignored=no=無視は除外、-c status.renames=true で改名検出を強制(ユーザ設定に依らず R を出す)。
    let out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // 背景ツールの掟: 任意ロック(index.lock の stat キャッシュ
            // 書き戻し)を取らない。konoma が status を回している最中の `git pull` を
            // 「index.lock: File exists」で失敗させない(git 2.15+)。
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
        // 各レコードは `XY` + 区切り空白 + パス。端数(末尾の空フィールド等)は捨てる。
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        let is_rename = x == b'R' || x == b'C' || y == b'R' || y == b'C';
        if is_rename {
            // 旧パスのフィールドを1つ消費して捨てる(状態は新パス=今のレコードに付ける)。
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
        FileStatus::Conflicted // 未マージ(衝突)
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
    use std::os::unix::ffi::OsStrExt;
    let mut set = HashSet::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return set; // repo でない / bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // `--ignored=traditional`: 無視エントリを `!!` で列挙し、**完全に無視されたディレクトリは
    //   collapse**(node_modules/ は中身を再帰せず1件)= git2 の recurse_ignored_dirs(false) と同じ。
    // `-unormal`: 未追跡 dir も collapse させるための既定モードを明示(ユーザの status.showUntrackedFiles
    //   が all だと無視 dir まで再帰展開され collapse が壊れるため、ここで固定する)。`??` 行は下で捨てる。
    let out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // 任意ロック禁止(statuses() と同じ理由)
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
        // 無視エントリは `!! <path>`。変更/未追跡(`??`)等の他レコードは捨てる。
        if rec.len() < 4 || rec[0] != b'!' || rec[1] != b'!' {
            continue;
        }
        // ignored ディレクトリは末尾 `/` 付きで来る→剥がしてツリーの絶対パスと一致させる。
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
    let repo = git2::Repository::discover(root).ok()?;
    let wd = repo.workdir()?.to_path_buf();
    Some(wd.canonicalize().unwrap_or(wd))
}

#[cfg(not(feature = "git"))]
pub fn workdir(_root: &Path) -> Option<PathBuf> {
    None
}

/// Returns the current branch name. A short hash when detached, or the HEAD reference name before the first commit (unborn).
/// Returns None when not a repo / when the feature is disabled.
#[cfg(feature = "git")]
pub fn branch(root: &Path) -> Option<String> {
    let repo = git2::Repository::discover(root).ok()?;
    if let Ok(head) = repo.head() {
        // git2 0.21: shorthand() は Result<&str, Error>(UTF-8 検証)。None 同等は .ok() で吸収。
        return head.shorthand().ok().map(|s| s.to_string());
    }
    // まだコミットが無い(unborn)場合は HEAD のシンボリック参照から取り出す。
    // git2 0.21: symbolic_target() は Result<Option<&str>, Error>。
    repo.find_reference("HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(|t| t.to_string()))
        .map(|t| t.trim_start_matches("refs/heads/").to_string())
}

#[cfg(not(feature = "git"))]
pub fn branch(_root: &Path) -> Option<String> {
    None
}

// ── diff / changed-files / log の読み取り＋書き込み API (Git view 用) ──────────
//
// 読み取りは git2、書き込み(stage/unstage/discard/commit)は `git` CLI に委譲する
// (フック・GPG 署名・ユーザー設定を尊重するため。書き込みに git2 は使わない)。
// `git` feature 無効時は全て空/None/Err を返し、機能以外は通常どおり動く。

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

/// For selecting which branches to show in the graph: returns local branches **ordered by most-recent tip commit**.
/// `(name, is_current, time_epoch)`. Ordered so the cap (`ui.graph_max_branches`) can take from the front.
#[cfg(feature = "git")]
pub fn branches_by_recency(root: &Path) -> Vec<(String, bool, i64)> {
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
            // tip コミット時刻(取得失敗は 0=最古扱い)。
            let t = branch
                .get()
                .peel_to_commit()
                .map(|c| c.time().seconds())
                .unwrap_or(0);
            out.push((name.to_string(), is_current, t));
        }
    }
    // 新しい順(降順)。同時刻は名前で安定化。
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
    // 安定ソート: HEAD(先頭) → 基準 → 残りは出現順を保つ。
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
    let Ok(repo) = git2::Repository::discover(root) else {
        return Vec::new();
    };
    let Some(workdir) = repo.workdir() else {
        return Vec::new();
    };
    let workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    // pathspec は workdir 相対で渡す。絶対パスでも strip_prefix で相対化を試みる。
    let file_abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = file_abs
        .strip_prefix(&workdir)
        .unwrap_or(&file_abs)
        .to_path_buf();
    let rel_str = rel.to_string_lossy().to_string();

    // HEAD ツリー(unborn なら None=空ツリー扱い)。
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        // 未追跡ファイルも行内容を出す(既定だと「未追跡」事実だけで行 diff が出ない)。
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
                    // ファイル境界ヘッダ: text=**素のパス**、行番号は両方 None(=ヘッダの目印)。
                    // 描画側(gitdiff)が枠付きヘッダにし、拡張子からファイル別ハイライトを切替える。
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
    use std::os::unix::ffi::OsStrExt;
    let mut out = Vec::new();
    let Some(workdir) = git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    else {
        return out; // repo でない / bare repo
    };
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    // porcelain v1 -z: NUL 区切りで各レコード `XY <path>`。改名/コピー(X か Y が R/C)のときは
    // 直後の NUL フィールドが旧パス(新パスが先=実測確認済)。-uall=未追跡 dir も再帰列挙、
    // --ignored=no=無視は除外、-c status.renames=true で改名検出を強制(ユーザ設定に依らず R を出す)。
    let cmd_out = std::process::Command::new("git")
        .current_dir(&workdir)
        .args([
            "--no-optional-locks", // 任意ロック禁止(statuses() と同じ理由)
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
        // 各レコードは `XY` + 区切り空白 + パス。端数(末尾の空フィールド等)は捨てる。
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        let is_rename = x == b'R' || x == b'C' || y == b'R' || y == b'C';
        if is_rename {
            // 旧パスのフィールドを1つ消費して捨てる(状態は新パス=今のレコードに付ける)。
            let _ = fields.next();
        }
        // staged = INDEX 側(X 列)に変更があるか。' '=index 変更なし / '?'=未追跡 / 'U'=未マージ は
        // staged 扱いしない(git2 版の INDEX_NEW/MODIFIED/DELETED/RENAMED/TYPECHANGE と同義)。
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
    let Ok(repo) = git2::Repository::discover(root) else {
        return out;
    };
    let Ok(mut walk) = repo.revwalk() else {
        return out;
    };
    if walk.push_head().is_err() {
        return out; // unborn 等
    }
    let _ = walk.set_sorting(git2::Sort::TIME);
    for oid in walk.flatten().take(max) {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let id = oid.to_string();
        let short = id.chars().take(7).collect();
        // git2 0.21: summary() は Result<Option<&str>, Error>。None/非 UTF-8 は空文字に。
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

/// Runs the `git` CLI in the repo workdir. A non-zero exit returns an Err carrying stderr.
#[cfg(feature = "git")]
fn run_git(root: &Path, args: &[&str]) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};
    let cwd = workdir_of(root);
    let out = std::process::Command::new("git")
        .current_dir(&cwd)
        .args(args)
        .output()
        .with_context(|| format!("git {} の起動に失敗", args.join(" ")))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(anyhow!("git {}: {}", args.join(" "), stderr.trim()))
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
    // %P=親ID(空白区切り)。topo-order=親を子より先に出さない＋枝が混ざらない(レーンが安定)。
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
        // 指定ブランチのみ。HEAD は呼び出し側で必ず含める(空グラフ回避)。
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
        // 先頭に %x1f があるので split で ["", H, P, h, s, an, ad, D]。
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

    // 未コミットの作業ツリー変更 (擬似 "Uncommitted changes" 行)。
    // **HEAD のコミットを親に持つ仮想コミットとして DAG に差し込む**ことで、レーン割当が
    // 自動で HEAD のレーン先頭に `●` を置き、HEAD まで `│` で繋ぐ(diff 先=HEAD と一致)。
    // HEAD が範囲内に無い/unborn の時は親無し=単独ノード(最左)へ degrade する。
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

    // 非基準レーンの循環パレット。基準(lane0)は固定色(下記 BASE)。
    const PALETTE: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Magenta,
        Color::Blue,
        Color::LightRed,
        Color::LightYellow,
    ];
    const BASE: Color = Color::White; // lane0=基準(左の幹)。テーマ前景寄りの固定色。

    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows: Vec<GraphRow> = Vec::new();

    // Phase 2: 基準指定時は lane0 を基準ブランチ先端で予約する。lane0 は以後、基準の first-parent を
    // 継ぎ続ける(=左の一直線)。他のコミット/新 tip/分岐は lane1 以降(右)へ追いやる(`base_floor`)。
    let base_floor = if let Some(tip) = base {
        lanes.push(Some(Lane {
            target: tip.to_string(),
            color: BASE,
        }));
        1
    } else {
        0
    };

    // 新レーンの色: lane0 は固定 BASE、それ以外はパレットを循環。
    let pick_color = |idx: usize, next: &mut usize| -> Color {
        if idx == 0 {
            BASE
        } else {
            let c = PALETTE[*next % PALETTE.len()];
            *next += 1;
            c
        }
    };
    // `start` 以降で最初の空きレーン番号(無ければ末尾に追加)。分岐は my_lane+1 以降=必ず右に出す。
    let free_from = |lanes: &mut Vec<Option<Lane>>, start: usize| -> usize {
        if let Some(i) = (start..lanes.len()).find(|&i| lanes[i].is_none()) {
            i
        } else {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    // コミット行(2桁/レーン)を組む。my_lane=ノード、他の活レーン=│、隙間=空白。
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
        // 1) このコミットが乗るレーン(最左の該当)。無ければ新 tip。
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
            // 新 tip。基準指定時は lane0 を避けて lane1 以降へ(基準の一直線を侵さない)。
            let idx = free_from(&mut lanes, base_floor);
            let col = pick_color(idx, &mut next_color);
            lanes[idx] = Some(Lane {
                target: c.id.clone(),
                color: col,
            });
            idx
        };
        let my_color = lanes[my_lane].as_ref().map(|l| l.color).unwrap_or(BASE);

        // 2) 合流(該当レーンが複数)はコミット行の**上**に連結行を挿み、余分なレーンを畳む。
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

        // 3) コミット行。
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

        // 4) 親を割り当てる。第1親=自レーン継続、第2親以降=右へ新レーン(=分岐)。
        let mut forked: Vec<(usize, Color)> = Vec::new();
        if c.parents.is_empty() {
            lanes[my_lane] = None; // ルート: レーン終端。
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

        // 5) 分岐はコミット行の**下**に連結行を挿む。
        if !forked.is_empty() {
            let conn = build_connector(&lanes, my_lane, my_color, &forked, true);
            rows.push(connector_row(conn));
        }

        // 6) 末尾の空レーンを詰める。
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
    }

    // 差し込んだ仮想 WT 行を本来の "Uncommitted changes" 擬似行へ戻す:
    // commit を外し worktree=true、ノード(●)を黄色ボールドに塗り直す(レーン位置は維持)。
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

    // 全行のグラフ幅を最大に揃え、subject の左端を一致させる。
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

// 連結文字を「接続方向ビット(上下左右)」から決めるための定数と表。
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

    // 1) 通過する縦レーン(my_lane でも endpoint でもない活レーン)= 上+下。
    for (i, l) in active.iter().enumerate() {
        if i == my_lane || endcols.contains(&i) {
            continue;
        }
        if let Some(l) = l {
            conn[i * 2] |= DIR_U | DIR_D;
            color[i * 2] = l.color;
        }
    }
    // 2) my_lane は上+下(継続)+右。
    conn[my_lane * 2] |= DIR_U | DIR_D | DIR_R;
    color[my_lane * 2] = my_color;
    // 3) 水平を my_lane の右隣から一番遠い endpoint まで一気に敷く(途中レーンを貫く)。
    for slot in conn[(my_lane * 2 + 1)..(max_e * 2)].iter_mut() {
        *slot |= DIR_L | DIR_R;
    }
    // 4) endpoint の角ビット。分岐=下+左 / 合流=上+左。手前の endpoint は水平が貫くので T(┬/┴)になる。
    let base = if is_fork {
        DIR_D | DIR_L
    } else {
        DIR_U | DIR_L
    };
    let mut sorted: Vec<(usize, Color)> = endpoints.to_vec();
    sorted.sort_by_key(|&(i, _)| i);
    let mut cursor = my_lane * 2; // 水平セルの色は、その右にある最も近い endpoint の色で塗る。
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

#[cfg(not(feature = "git"))]
pub fn stage(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn unstage(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn stage_all(_root: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn unstage_all(_root: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn discard(_root: &Path, _file: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn commit(_root: &Path, _message: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn checkout(_root: &Path, _name: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn create_branch(_root: &Path, _name: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
}

#[cfg(not(feature = "git"))]
pub fn delete_branch(_root: &Path, _name: &str, _force: bool) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("git feature 無効"))
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

    /// 背景の読み取り(statuses/ignored)は index を**書き戻さない**(--no-optional-locks)。
    /// stat キャッシュが陳腐化した状態(内容同一で mtime だけ更新)で status を読んでも
    /// .git/index が変化しないこと。これが破れると、konoma が status を回している最中の
    /// `git pull` が「index.lock: File exists」で失敗する(ユーザー報告 2026-07-07)。
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

        // 内容同一のまま mtime を更新 → index の stat キャッシュが陳腐化。
        // フラグ無しの `git status` はここで index を書き戻す(=index.lock を取る)。
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("a.txt"), b"same content").unwrap();

        let index = dir.join(".git/index");
        let before = std::fs::metadata(&index).unwrap().modified().unwrap();
        let st = statuses(&dir);
        let ig = ignored(&dir);
        // git2 の gutter diff(diff_tree_to_workdir_with_index)も update_index を立てないので
        // index を書き戻さない。これが背景 git 読み取りをロックフリーに保つ前提であり、
        // それゆえ `.git/*.lock` イベントを握り潰さずに済む(fs 監視の自己ループが起きない)。
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

    #[cfg(feature = "git")]
    #[test]
    fn lay_out_lanes_draws_angular_fork_and_merge() {
        // M(merge B,F) → B(→R) / F(→R) → R(root)。分岐=コミット下に ├─┐、合流=コミット上に ├─┘。
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
        // 斜め線が一切無いこと(角ばったTUIと整合)。
        let all: String = rows
            .iter()
            .flat_map(|r| r.graph.iter().map(|(s, _)| s.clone()))
            .collect();
        assert!(
            !all.contains('/') && !all.contains('\\'),
            "斜め線が残っている: {all}"
        );
        // ノード: マージ=◆(1個)・通常=●(3個)。コミット行は 4。
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
        // 3つの tip が同じ root R に合流 → 中間レーンは ┴(上+左+右)、最遠は ┘。`├───┘` にならないこと。
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
        // 2本の枝(A: A1→A2→R / B: B1→B2→R)。base 指定でその枝の node が必ず lane0(col0)に来る。
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
        // コミット id の node が col0(lane0)に乗っているか。
        let node_at_col0 = |rows: &[GraphRow], id: &str| -> bool {
            rows.iter().any(|r| {
                r.commit.as_deref() == Some(id)
                    && matches!(r.graph.first(), Some((s, _)) if s == "●" || s == "◆")
            })
        };

        // base なし: 先に現れる A が lane0、B は右。
        let none = lay_out_lanes(&commits, None, None);
        assert!(node_at_col0(&none, "A1"), "base なしでは A1 が lane0");
        assert!(
            !node_at_col0(&none, "B1"),
            "base なしでは B1 は lane0 でない"
        );

        // base=B1(feature 先端): B 系が lane0 へ、A は右に追いやられる。
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
        // 全コミット行は維持(消えない)。
        assert_eq!(
            based.iter().filter(|r| r.commit.is_some()).count(),
            5,
            "全コミットが残る(--all 相当)"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn worktree_row_sits_on_head_lane_not_always_col0() {
        // HEAD=A1(枝A の先端)。base=B1 で lane0 は B 系。未コミット行は **A のレーン(col0 でない)** に乗り、
        // HEAD の直上に縦で繋がること(diff 先=HEAD と一致)。
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

        // base=B1: lane0=B 系。WT 行は worktree=true・col0 が `●` ではない(HEAD=A は右レーン)。
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
        // WT のノード列が HEAD(A1)のノード列と一致(縦に繋がる)。
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

        // base なし: HEAD=A は最左 lane0 なので WT も col0(従来表示と一致)。
        let rows0 = lay_out_lanes(&commits, None, wt);
        let wt0 = rows0.iter().find(|r| r.worktree).unwrap();
        assert!(
            matches!(wt0.graph.first(), Some((s, _)) if s == "●"),
            "base なしでは WT は col0(HEAD=lane0)"
        );
    }

    #[test]
    fn marker_and_rank_are_distinct() {
        // マーカーは種別ごとに一意。
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
        // 畳み込み優先度: conflicted が最強、untracked が最弱。
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
        // 未追跡ファイルが Untracked。
        assert_eq!(
            map.get(&canon.join("sub").join("foo.txt")),
            Some(&FileStatus::Untracked)
        );
        // 親ディレクトリにも畳み込まれる。
        assert_eq!(map.get(&canon.join("sub")), Some(&FileStatus::Untracked));
        std::fs::remove_dir_all(&dir).ok();
    }

    // CLI 委譲後も改名が「新パスに Renamed」で出ること(porcelain -z は new→old 順)を固定。
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
        // git mv で改名してステージ。
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(["mv", "old.txt", "new.txt"])
            .output()
            .unwrap();

        let map = statuses(&dir);
        let canon = dir.canonicalize().unwrap();
        // 新パスに Renamed、旧パスは出ない。
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
        // 無視 dir は collapse(中身を再帰せず dir 1件)。recurse_ignored_dirs(false) 相当。
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
        // collapse なので中身の個別ファイルは入らない。
        assert!(
            !set.contains(&canon.join("target/a.o")),
            "collapse 中の個別ファイルは入らない: {set:?}"
        );
        // 追跡ファイル/.gitignore 自身は無視ではない。
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
        // 初期コミット。
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
        // 変更。
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
        // ステージ前は untracked・staged=false。
        let before = changed_files(&dir);
        assert_eq!(before.len(), 1);
        assert!(!before[0].staged);
        // git add でステージ → staged=true。
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
        // 公開 API でステージ → コミット。
        stage(&dir, &f).unwrap();
        commit(&dir, "first commit").unwrap();
        let entries = log(&dir, 10);
        assert_eq!(entries.len(), 1, "log は1件のはず");
        assert_eq!(entries[0].summary, "first commit");
        assert_eq!(entries[0].short.len(), 7);
        // commit_diff は空でない(新規ファイル追加)。
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
        // 変更してステージ → unstage で index から外れる。
        std::fs::write(&f, b"v2\n").unwrap();
        stage(&dir, &f).unwrap();
        assert!(changed_files(&dir).iter().any(|e| e.staged));
        unstage(&dir, &f).unwrap();
        assert!(
            !changed_files(&dir).iter().any(|e| e.staged),
            "unstage 後は staged が無いはず"
        );
        // discard で作業ツリーの変更を破棄 → クリーンに。
        discard(&dir, &f).unwrap();
        assert!(changed_files(&dir).is_empty(), "discard 後はクリーンのはず");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    // SPEED GUARD: lay_out_lanes は DAG(親IDリスト)からレーンを割り当てる純関数。1000 件の
    // 線形チェーンでも O(n) 程度で収まること(レーン割当が二乗化していないかの回帰ガード)。
    // 実 repo 不要なので合成 DAG を直接渡す。生成は安いので非 ignore。
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
