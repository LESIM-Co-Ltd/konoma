//! File operation primitives (M7 Phase B). Destructive operations default to **a confirmation dialog plus moving to Trash** (safety first).
//! This module handles only fs operations; the confirmation UI and key handling live on the app/ui side.
//! The principles are: never overwrite existing entries (create/rename error on collision) and keep deletion recoverable (Trash).

use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Errors `fileops` returns for conditions the UI wants to word for the user (create/rename
/// collisions, Trash failures, batch-rename issues). `fileops` deliberately doesn't know the UI
/// language (threading `Lang` through every signature here for a handful of conditions would be
/// invasive for a "language-agnostic" module) — the translation step lives on the `App` side, in
/// `describe_error`/`describe_file_op_error` (`src/app/file_actions.rs`), which downcasts an
/// `anyhow::Error` back to this type. That works whether a variant is the top-level error
/// (`anyhow::bail!(FileOpError::…)`) or was attached as `.context(FileOpError::…)` around a lower
/// io error: anyhow's downcast checks both the context value and the wrapped error (see
/// `anyhow::Context`'s docs, "Effect on downcasting"), so the wrapped io error stays reachable via
/// `source()`/`chain()` either way — this module never has to discard it to make the type carry a
/// translatable label.
///
/// Every other error condition in this module (a plain io failure while creating a parent
/// directory, reading a symlink, etc.) stays a plain English `.context(...)` string: those are
/// low-value, rarely-hit implementation detail, not something a Japanese-reading user needs
/// worded in Japanese, so typing them would be needless enum sprawl. Falls back to `Display` (see
/// below) unmodified when it doesn't match any arm below (see `describe_error`'s fallback).
#[derive(Debug)]
pub enum FileOpError {
    /// `create_file`/`create_dir`/`rename` (single-file rename): the target name already exists.
    AlreadyExists(PathBuf),
    /// `move_to_trash`: the OS trash service failed (e.g. no Trash support in this environment).
    /// The underlying error from the `trash` crate is kept as `source()`.
    TrashFailed,
    /// `batch_rename`: the temp staging name (`.konoma-rename-tmp-N`) already exists — this would
    /// require a file already using that exact reserved name, so it's effectively unreachable in
    /// practice, but the check (and this variant) exist as a defensive belt-and-braces.
    RenameTempExists(PathBuf),
    /// `batch_rename`: the final destination name already exists. A defensive re-check — the
    /// caller (`App::build_rename_plan`) already validates this before showing the preview — so
    /// this only fires if the filesystem changed out from under the plan between preview and apply.
    RenameDestExists(PathBuf),
    /// `copy_into_with_progress`/`move_into_with_progress`: `src.file_name()` returned nothing
    /// usable (e.g. `src` is `/` or ends in `..`).
    NameUnavailable(PathBuf),
    /// `batch_rename`: the `rename()` call itself failed (e.g. permission denied) while staging
    /// `src` aside to its temp name. The underlying io error is kept as `source()`.
    RenameStageFailed(PathBuf),
    /// `batch_rename`: the `rename()` call itself failed while committing the temp name to `dst`.
    /// The underlying io error is kept as `source()`.
    RenameCommitFailed(PathBuf),
}

/// English only — for logs/tests/developers. The UI-facing (localized) text is built separately by
/// `describe_error`/`describe_file_op_error`, which match on the variant instead of using this.
impl std::fmt::Display for FileOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileOpError::AlreadyExists(p) => write!(f, "already exists: {}", p.display()),
            FileOpError::TrashFailed => write!(f, "failed to move to Trash"),
            FileOpError::RenameTempExists(p) => {
                write!(f, "temporary rename name already exists: {}", p.display())
            }
            FileOpError::RenameDestExists(p) => {
                write!(f, "rename destination already exists: {}", p.display())
            }
            FileOpError::NameUnavailable(p) => {
                write!(f, "could not determine a name: {}", p.display())
            }
            FileOpError::RenameStageFailed(p) => write!(f, "staging rename: {}", p.display()),
            FileOpError::RenameCommitFailed(p) => write!(f, "committing rename: {}", p.display()),
        }
    }
}

impl std::error::Error for FileOpError {}

/// Attached by `rollback_batch_rename` as **anyhow context** when the automatic rollback could not
/// put everything back, so entries are left sitting under their staging name
/// (`.konoma-rename-tmp-N`) or under their new name.
///
/// It is a distinct type rather than another `FileOpError` variant on purpose: the UI resolves the
/// *reason* the rename failed with `downcast_ref::<FileOpError>()`, and folding this into that enum
/// would make the outermost context win that lookup and hide the reason. As its own type, the two
/// downcasts are independent, so `describe_file_op_error` can report both — the reason **and** the
/// fact that files were left behind (which the user has to clean up by hand, so silently dropping
/// it is the one thing we must not do).
#[derive(Debug)]
pub struct RollbackIncomplete {
    /// Where the entries that could not be restored actually are right now (staging names, or the
    /// new name for one that was already committed). Never empty.
    pub leftovers: Vec<PathBuf>,
}

/// English only, same as `FileOpError` — the UI-facing localized text is built by
/// `describe_file_op_error`.
impl std::fmt::Display for RollbackIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rollback incomplete, left behind: ")?;
        for (i, p) in self.leftovers.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", p.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for RollbackIncomplete {}

/// File information (for the `i` popup). For a symlink, holds the link's own info plus its target.
pub struct FileInfo {
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Byte size (file = actual size / directory = metadata len).
    pub size: u64,
    /// unix st_mode (including the type bits). Permissions are the low 9 bits.
    pub mode: u32,
    /// Modification time (UNIX epoch seconds). None if unavailable.
    pub modified_epoch: Option<u64>,
    /// The symlink's target.
    pub symlink_target: Option<PathBuf>,
    /// Number of direct entries in the directory (None for files).
    pub child_count: Option<usize>,
}

/// Retrieve metadata for `path` (does not follow symlinks; inspects the link itself).
pub fn file_info(path: &Path) -> Result<FileInfo> {
    let meta =
        std::fs::symlink_metadata(path).with_context(|| format!("get info: {}", path.display()))?;
    let is_symlink = meta.file_type().is_symlink();
    let is_dir = meta.is_dir();
    let modified_epoch = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let child_count = if is_dir {
        std::fs::read_dir(path).ok().map(|rd| rd.count())
    } else {
        None
    };
    Ok(FileInfo {
        is_dir,
        is_symlink,
        size: meta.len(),
        mode: meta.permissions().mode(),
        modified_epoch,
        symlink_target: if is_symlink {
            std::fs::read_link(path).ok()
        } else {
            None
        },
        child_count,
    })
}

/// Format a byte count as human-readable (base 1024, "B/KB/MB/GB/TB/PB").
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// Just the rwx part of permissions, "rwxr-xr-x" (low 9 bits, for column display).
pub fn permission_rwx(mode: u32) -> String {
    let bits = mode & 0o777;
    let g = |b: u32| {
        format!(
            "{}{}{}",
            if b & 0b100 != 0 { "r" } else { "-" },
            if b & 0b010 != 0 { "w" } else { "-" },
            if b & 0b001 != 0 { "x" } else { "-" },
        )
    };
    format!(
        "{}{}{}",
        g((bits >> 6) & 7),
        g((bits >> 3) & 7),
        g(bits & 7)
    )
}

/// Permissions in "rwxr-xr-x (755)" form (low 9 bits).
pub fn permission_string(mode: u32) -> String {
    let bits = mode & 0o777;
    let rwx = |b: u32| {
        format!(
            "{}{}{}",
            if b & 0b100 != 0 { "r" } else { "-" },
            if b & 0b010 != 0 { "w" } else { "-" },
            if b & 0b001 != 0 { "x" } else { "-" },
        )
    };
    format!(
        "{}{}{} ({:03o})",
        rwx((bits >> 6) & 7),
        rwx((bits >> 3) & 7),
        rwx(bits & 7),
        bits
    )
}

/// Convert UNIX epoch seconds to "YYYY-MM-DD HH:MM:SS UTC" (calendar math with no extra dependency, Howard Hinnant's method).
pub fn format_epoch_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days: days since 1970-01-01 → calendar date.
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Short date-time for column display, "YYYY-MM-DD HH:MM" (UTC, no seconds/label).
pub fn format_epoch_short(secs: u64) -> String {
    format_epoch_utc(secs)[..16].to_string()
}

/// Lightweight per-row metadata for column display (`symlink_metadata` only; no `read_dir`; does not follow symlinks).
pub struct RowMeta {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mtime: Option<u64>,
    pub mode: u32,
}

/// Get the lightweight row metadata (None on failure).
pub fn quick_meta(path: &Path) -> Option<RowMeta> {
    let m = std::fs::symlink_metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Some(RowMeta {
        is_dir: m.is_dir(),
        is_symlink: m.file_type().is_symlink(),
        size: m.len(),
        mtime,
        mode: m.permissions().mode(),
    })
}

/// Fixed width of the detail-list column `id` (for the `[ui] details` setting). None for an unknown id.
pub fn detail_column_width(id: &str) -> Option<usize> {
    match id {
        "size" => Some(9),
        "modified" | "mtime" => Some(16),
        "perm" | "permissions" => Some(9),
        "type" => Some(4),
        "items" => Some(6),
        _ => None,
    }
}

/// One cell string for the detail list (the value before right-alignment). None if `id` is unknown.
/// Only `items` does a `read_dir` on the directory (the others need only `m`).
pub fn detail_cell(id: &str, path: &Path, m: &RowMeta) -> Option<String> {
    Some(match id {
        "size" => {
            if m.is_dir {
                "--".to_string()
            } else {
                human_size(m.size)
            }
        }
        "modified" | "mtime" => m.mtime.map(format_epoch_short).unwrap_or_default(),
        "perm" | "permissions" => permission_rwx(m.mode),
        "type" => if m.is_symlink {
            "link"
        } else if m.is_dir {
            "dir"
        } else {
            "file"
        }
        .to_string(),
        "items" => {
            if m.is_dir {
                std::fs::read_dir(path)
                    .map(|rd| rd.count().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        _ => return None,
    })
}

/// Live counters shared with the UI while a background file operation runs.
/// Both are monotonic; the UI reads them each frame (no locking — `Relaxed` is enough since
/// these are just a progress readout, never used to synchronize other state).
#[derive(Debug, Default)]
pub struct Progress {
    items: std::sync::atomic::AtomicUsize, // count of finished top-level targets (one paste/duplicate entry = one file or one directory)
    files: std::sync::atomic::AtomicUsize, // running count of leaf entries (files/symlinks actually copied or moved)
}

impl Progress {
    /// Record one leaf file (or symlink) copied/moved.
    pub fn bump_file(&self) {
        self.files
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    /// Record one top-level target finished.
    pub fn bump_item(&self) {
        self.items
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    /// Leaf files copied/moved so far.
    pub fn files(&self) -> usize {
        self.files.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Top-level targets finished so far.
    pub fn items(&self) -> usize {
        self.items.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Move multiple paths to Trash (recoverable). On macOS this is the proper Trash (supports Finder's "Put Back").
pub fn move_to_trash(paths: &[PathBuf]) -> Result<()> {
    trash::delete_all(paths).context(FileOpError::TrashFailed)?;
    Ok(())
}

/// After a **failed** `move_to_trash`, work out what actually happened rather than assuming
/// "nothing" — `trash::delete_all` doesn't report which (if any) targets it got to before the
/// error, and different Trash backends behave differently here: macOS's default `Finder`
/// AppleScript method batches the whole list into a single `osascript` call (so one bad path can
/// fail the entire command before anything is removed), while the `NsFileManager` method (and the
/// freedesktop backend on Linux) loops per path and can succeed on several targets before hitting
/// one that fails. Either way, the ground truth is the filesystem, not the library's error: a
/// target that no longer exists was actually trashed, regardless of which backend or code path did
/// it. `symlink_metadata` (not `metadata`) so a target that is itself a symlink is judged by
/// whether the link was removed, not whether whatever it pointed at still exists.
///
/// Returns `(how many actually disappeared, the first target that is still there)` — the caller
/// uses the count for an honest "N succeeded" message and the still-present path to give the user
/// something concrete to act on instead of just "failed" (see `App::run_file_op`'s `Trash` arm).
pub fn trash_partial_outcome(targets: &[PathBuf]) -> (usize, Option<&PathBuf>) {
    let still_present: Vec<&PathBuf> = targets
        .iter()
        .filter(|t| std::fs::symlink_metadata(t).is_ok())
        .collect();
    let ok = targets.len() - still_present.len();
    (ok, still_present.first().copied())
}

/// **Permanently delete** multiple paths (unrecoverable; does not go through Trash). Directories are deleted with their contents.
/// For a symlink, only the link itself is removed (the target is not followed).
///
/// The production call site is [`delete_permanently_with_progress`] (this is the same thing with a
/// throwaway [`Progress`]); as with [`copy_into`], the plain form is kept only so the pre-existing
/// unit tests below did not need to change their call sites, hence `cfg(test)`.
#[cfg(test)]
fn delete_permanently(paths: &[PathBuf]) -> Result<()> {
    delete_permanently_with_progress(paths, &Progress::default())
}

/// Same as [`delete_permanently`], but bumps `p` **per path** so the background file-op worker can
/// report real progress (`3/12`) instead of sitting at `0/12` until the whole batch is done.
/// (`move_to_trash` cannot do this: it is a single `trash::delete_all` call for the whole batch.)
pub fn delete_permanently_with_progress(paths: &[PathBuf], p: &Progress) -> Result<()> {
    for path in paths {
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("get info: {}", path.display()))?;
        if meta.is_dir() {
            std::fs::remove_dir_all(path)
                .with_context(|| format!("permanently delete directory: {}", path.display()))?;
        } else {
            std::fs::remove_file(path)
                .with_context(|| format!("permanently delete: {}", path.display()))?;
        }
        p.bump_item();
    }
    Ok(())
}

/// Create an empty file directly under `dir` (or in a hierarchy included in `name`). Errors if it already exists (does not overwrite).
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        anyhow::bail!(FileOpError::AlreadyExists(path));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent directory: {}", parent.display()))?;
    }
    std::fs::File::create(&path).with_context(|| format!("create file: {}", path.display()))?;
    Ok(path)
}

/// Create a directory directly under `dir` (also creating intermediate directories). Errors if it already exists.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        anyhow::bail!(FileOpError::AlreadyExists(path));
    }
    std::fs::create_dir_all(&path)
        .with_context(|| format!("create directory: {}", path.display()))?;
    Ok(path)
}

/// Rename `src` to `new_name` within the same parent directory. Errors if the new name already exists (does not overwrite).
/// The same name (no change) succeeds as a no-op.
pub fn rename(src: &Path, new_name: &str) -> Result<PathBuf> {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let dst = parent.join(new_name);
    if dst == src {
        return Ok(dst);
    }
    if dst.exists() {
        anyhow::bail!(FileOpError::AlreadyExists(dst));
    }
    std::fs::rename(src, &dst).with_context(|| format!("rename: {}", dst.display()))?;
    Ok(dst)
}

/// Expand the sequential-rename template for a single file (pure function).
/// Placeholders: `{n}` = sequence number / `{n:0W}` = zero-padded to width W / `{name}` = name without extension / `{ext}` = extension (no dot).
/// Unknown tokens are emitted as-is (`{token}`) so typos are noticeable. Automatic extension completion is the caller's job.
pub fn render_rename_template(template: &str, n: usize, stem: &str, ext: &str) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < template.len() {
        // `{` is ASCII (0x7B). It never appears mid-way through a UTF-8 multi-byte character, so a
        // byte check is safe.
        if bytes[i] == b'{' {
            if let Some(rel) = template[i + 1..].find('}') {
                let token = &template[i + 1..i + 1 + rel];
                match render_token(token, n, stem, ext) {
                    Some(s) => out.push_str(&s),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                i = i + 1 + rel + 1;
                continue;
            }
        }
        let ch = template[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Expand the contents of `{...}`. Unsupported tokens return None (treated as a literal).
fn render_token(token: &str, n: usize, stem: &str, ext: &str) -> Option<String> {
    match token {
        "n" => Some(n.to_string()),
        "name" => Some(stem.to_string()),
        "ext" => Some(ext.to_string()),
        // `{n:0W}` = a zero-padded sequence number of width W.
        _ => token
            .strip_prefix("n:0")
            .and_then(|w| w.parse::<usize>().ok())
            .map(|w| format!("{n:0w$}")),
    }
}

/// Bulk rename (collision-safe, two-phase). Each `(src,dst)` is first moved aside to a temporary name, then to its final name.
/// `src==dst` (no change) is skipped. This keeps swaps like `a→b, b→a` from breaking.
/// Final-name collision checking is assumed to be done by the caller beforehand, but the existence of the temporary/final names is also checked and the operation aborts on the safe side.
///
/// **It restores the original state no matter which phase fails**: completed operations are recorded, and on failure they are rolled back
/// best-effort in reverse order before returning the **original error**. Secondary failures during rollback are appended to the context.
/// This prevents staged files (`.konoma-rename-tmp-*`) from being orphaned so that data appears to have been lost.
pub fn batch_rename(plan: &[(PathBuf, PathBuf)]) -> Result<()> {
    // What was set aside in the first loop: (temp name tmp, original src, final dst).
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
    // The count committed in the second loop (counted from the front of staged). `staged[..committed]`
    // has already been committed.
    let mut committed = 0usize;

    for (i, (src, dst)) in plan.iter().enumerate() {
        if src == dst {
            continue;
        }
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(".konoma-rename-tmp-{i}"));
        if tmp.exists() {
            let err = anyhow::Error::new(FileOpError::RenameTempExists(tmp));
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        if let Err(e) = std::fs::rename(src, &tmp) {
            let err = anyhow::Error::new(e).context(FileOpError::RenameStageFailed(src.clone()));
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        staged.push((tmp, src.clone(), dst.clone()));
    }

    for idx in 0..staged.len() {
        let (tmp, _src, dst) = &staged[idx];
        if dst.exists() {
            let err = anyhow::Error::new(FileOpError::RenameDestExists(dst.clone()));
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        if let Err(e) = std::fs::rename(tmp, dst) {
            let err = anyhow::Error::new(e).context(FileOpError::RenameCommitFailed(dst.clone()));
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        committed += 1;
    }
    Ok(())
}

/// Rollback for `batch_rename`. Reverts the operations done so far best-effort and returns `err` (the first error).
/// **It reverts in two phases, like the forward pass**: reverting directly with `dst→src` would, when a swap/cycle is already committed,
/// have each element occupying the other's `src`, so `dst→src` would overwrite a live file and corrupt data.
/// So it (1) first moves the committed entries (`staged[..committed]`) aside with `dst→tmp` to gather all staged entries at tmp →
/// (2) reverts all staged with `tmp→src` (at which point all srcs are free and do not collide).
/// Secondary failures during rollback are appended to the original error's context (but the first error takes precedence as the return value).
fn rollback_batch_rename(
    mut err: anyhow::Error,
    staged: &[(PathBuf, PathBuf, PathBuf)],
    committed: usize,
) -> anyhow::Error {
    // 1) Move the already-committed entries back from dst → tmp. The tmp name
    //    (`.konoma-rename-tmp-{i}`) is unused, so there's no collision.
    //    This lines every staged entry up at tmp (together with the not-yet-committed ones, which
    //    are already at tmp), so a swap/cycle can no longer collide by overwriting each other's
    //    src. Process in reverse order to unwind the forward commit order.
    for (tmp, _src, dst) in staged[..committed].iter().rev() {
        if let Err(e) = std::fs::rename(dst, tmp) {
            err = err.context(format!(
                "rollback failed (unstage committed {} → {}): {e}",
                dst.display(),
                tmp.display()
            ));
        }
    }
    // 2) Move every staged entry back from tmp → src. Every src is now free, so a swap/cycle
    //    can't collide either.
    for (tmp, src, _dst) in staged.iter().rev() {
        if let Err(e) = std::fs::rename(tmp, src) {
            err = err.context(format!(
                "rollback failed (restore {} → {}): {e}",
                tmp.display(),
                src.display()
            ));
        }
    }
    // 3) Report what is actually left behind. **Observed from the filesystem** rather than inferred
    //    from which rename calls failed (same approach as `trash_partial_outcome`): the two phases
    //    interact — an entry whose phase-1 `dst→tmp` failed then also fails phase 2, which would
    //    otherwise be counted twice and name a `tmp` path that does not exist. Asking the disk
    //    where each entry ended up cannot get that wrong.
    //    The string contexts above stay for logs; this typed one is what the UI reads, because a
    //    string context is invisible to `describe_file_op_error`'s downcast (that is the bug this
    //    fixes: the user was told *why* the rename failed but not that `.konoma-rename-tmp-*`
    //    files were still lying around).
    //    A staging name still existing is unambiguous — `.konoma-rename-tmp-N` is ours and nothing
    //    else creates it. Otherwise the entry counts as stranded only if its original name is gone
    //    *and* the new name is occupied, i.e. phase 1 could not unstage it. Deliberately not
    //    "src is missing" on its own: something else may legitimately occupy `src` (the very thing
    //    that can block the restore), and an entry whose staged file was deleted out from under us
    //    has genuinely left nothing behind to clean up.
    let leftovers: Vec<PathBuf> = staged
        .iter()
        .filter_map(|(tmp, src, dst)| {
            if tmp.symlink_metadata().is_ok() {
                Some(tmp.clone())
            } else if src.symlink_metadata().is_err() && dst.symlink_metadata().is_ok() {
                Some(dst.clone())
            } else {
                None
            }
        })
        .collect();
    if !leftovers.is_empty() {
        err = err.context(RollbackIncomplete { leftovers });
    }
    err
}

/// Return a non-colliding name for `name` within `dir`. If it exists, append ` copy` / ` copy N` while keeping the extension.
/// A helper **for not overwriting existing entries** (deciding the destination name for copy/move).
fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|s| s.to_str());
    let make = |suffix: &str| match ext {
        Some(e) => format!("{stem}{suffix}.{e}"),
        None => format!("{stem}{suffix}"),
    };
    let mut cand = make(" copy");
    let mut i = 2;
    while dir.join(&cand).exists() {
        cand = make(&format!(" copy {i}"));
        i += 1;
    }
    cand
}

/// **Recreate** the symlink at `dst` (does not follow the link; duplicates the target path as-is).
/// Used on every copy/move path to avoid materializing a symlink→file or following a symlink→dir.
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    let target =
        std::fs::read_link(src).with_context(|| format!("read link: {}", src.display()))?;
    std::os::unix::fs::symlink(&target, dst)
        .with_context(|| format!("create link: {}", dst.display()))?;
    Ok(())
}

/// Recursively copy a directory (done by hand since `std::fs` lacks it). `dst` is newly created.
/// Symlinks are duplicated as links and their targets are not followed (to prevent accidental materialization/following).
/// `p` is bumped once per leaf file and per symlink recreated, so a caller polling it from another
/// thread sees the counter advance **inside** a large directory instead of jumping only at the end.
fn copy_dir_all(src: &Path, dst: &Path, p: &Progress) -> Result<()> {
    // Self-containment guard: if `dst` is inside the `src` subtree, read_dir keeps picking up the
    // growing `dst` and recurses forever (e.g. dragging a parent folder into its child in Finder).
    // Symmetric to paste()'s starts_with guard — defend every copy/move/drop path here in one place.
    if dst.starts_with(src) {
        anyhow::bail!(
            "copy destination is inside the copy source: {} ⊂ {}",
            dst.display(),
            src.display()
        );
    }
    std::fs::create_dir_all(dst).with_context(|| format!("create: {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read: {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            copy_symlink(&from, &to)?;
            p.bump_file();
        } else if ft.is_dir() {
            copy_dir_all(&from, &to, p)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("copy: {}", from.display()))?;
            p.bump_file();
        }
    }
    Ok(())
}

/// **Copy** `src` (file/directory) directly under `dir`. On collision, append ` copy` and **do not overwrite**.
/// Copying into the same directory produces a duplicate (`name copy`). Returns the resolved destination path.
///
/// Every production call site now goes through [`copy_into_with_progress`] (which this just wraps with a
/// throwaway [`Progress`]) so the background file-op worker can report live progress; this plain form is
/// kept **only** so the pre-existing unit tests below did not need to change their call sites, hence `cfg(test)`.
#[cfg(test)]
fn copy_into(dir: &Path, src: &Path) -> Result<PathBuf> {
    copy_into_with_progress(dir, src, &Progress::default())
}

/// Same as [`copy_into`], but bumps `p` as leaf files are copied — for the background file-op
/// worker to report live progress on a directory that may contain many files.
pub fn copy_into_with_progress(dir: &Path, src: &Path, p: &Progress) -> Result<PathBuf> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context(FileOpError::NameUnavailable(src.to_path_buf()))?;
    let dst = dir.join(unique_name(dir, name));
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, &dst)?;
        p.bump_file();
    } else if meta.is_dir() {
        copy_dir_all(src, &dst, p)?;
    } else {
        std::fs::copy(src, &dst).with_context(|| format!("copy: {}", src.display()))?;
        p.bump_file();
    }
    Ok(dst)
}

/// **Move** `src` directly under `dir`. Moving into the same directory is a no-op. On collision, append ` copy` and do not overwrite.
/// If rename is impossible (e.g. across volumes), fall back to copy plus deleting the original. Returns the resolved destination path.
///
/// Kept only for the pre-existing unit tests below (see [`copy_into`]) — production code calls
/// [`move_into_with_progress`] directly.
#[cfg(test)]
fn move_into(dir: &Path, src: &Path) -> Result<PathBuf> {
    move_into_with_progress(dir, src, &Progress::default())
}

/// Same as [`move_into`], but bumps `p` as leaf files are moved — for the background file-op
/// worker to report live progress on a directory that may contain many files.
pub fn move_into_with_progress(dir: &Path, src: &Path, p: &Progress) -> Result<PathBuf> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context(FileOpError::NameUnavailable(src.to_path_buf()))?;
    let same = dir.join(name);
    if same == src {
        return Ok(same); // already in the same place
    }
    let dst = dir.join(unique_name(dir, name));
    if std::fs::rename(src, &dst).is_ok() {
        p.bump_file();
        return Ok(dst);
    }
    // Different volume, etc. → copy then delete the original. A symlink duplicates the link
    // itself and does not follow its target.
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, &dst)?;
        std::fs::remove_file(src)?;
        p.bump_file();
    } else if meta.is_dir() {
        copy_dir_all(src, &dst, p)?;
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::copy(src, &dst).with_context(|| format!("move (copy): {}", src.display()))?;
        std::fs::remove_file(src)?;
        p.bump_file();
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    #[test]
    fn copy_into_self_subtree_is_rejected_not_infinite() {
        // Copy a parent folder into its own child (e.g. a Finder drop). Without the
        // self-containment guard, read_dir keeps picking up the growing copy destination and
        // generates `/parent/child/parent/child/…` forever. Confirm the guard makes this fail
        // immediately with "the copy destination is inside the copy source" (D1).
        let root = unique_tmp("konoma_selfcopy_test");
        let _ = std::fs::remove_dir_all(&root);
        let parent = root.join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(parent.join("f.txt"), b"x").unwrap();

        // Copy parent into child → self-contained, so Err (does not recurse forever).
        let err = copy_into(&child, &parent).unwrap_err();
        assert!(
            err.to_string().contains("inside the copy source"),
            "自己包含エラーであるべき: {err}"
        );
        // No infinite nesting was created (parent was not grown under child).
        assert!(
            !child.join("parent").join("parent").exists(),
            "増殖したネストが残ってはならない"
        );

        // move_into goes through the same guard too (the rename-fails → copy_dir_all path). But
        // in an environment where rename succeeds it would just move, so this test only targets
        // the copy path.

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_and_rename_respect_existing() {
        let dir = unique_tmp("konoma_fileops_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create.
        let f = create_file(&dir, "a.txt").unwrap();
        assert!(f.is_file());
        // An existing one errors instead of being overwritten.
        assert!(
            create_file(&dir, "a.txt").is_err(),
            "既存ファイルは上書きしない"
        );
        // Nested creation (sub/b.txt).
        let nested = create_file(&dir, "sub/b.txt").unwrap();
        assert!(nested.is_file());
        // Create a directory.
        let d = create_dir(&dir, "newdir").unwrap();
        assert!(d.is_dir());
        assert!(
            create_dir(&dir, "newdir").is_err(),
            "既存ディレクトリはエラー"
        );

        // Rename.
        let renamed = rename(&f, "a2.txt").unwrap();
        assert!(renamed.is_file() && !f.exists());
        assert_eq!(renamed.file_name().unwrap(), "a2.txt");
        // The same name succeeds as a no-op.
        assert!(rename(&renamed, "a2.txt").is_ok());
        // Renaming to an existing name errors (does not overwrite).
        create_file(&dir, "c.txt").unwrap();
        assert!(rename(&renamed, "c.txt").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_permanently_removes_file_and_dir() {
        let dir = unique_tmp("konoma_perm_delete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Permanently delete a file and a directory (with contents).
        let f = dir.join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"y").unwrap();

        delete_permanently(&[f.clone(), sub.clone()]).unwrap();
        assert!(!f.exists(), "ファイルが完全削除される");
        assert!(!sub.exists(), "ディレクトリが中身ごと完全削除される");
        // A nonexistent path errors.
        assert!(delete_permanently(std::slice::from_ref(&f)).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_rename_template_expands_placeholders() {
        assert_eq!(
            render_rename_template("{name}_{n}", 1, "foo", "jpg"),
            "foo_1"
        );
        assert_eq!(
            render_rename_template("{name}_{n:03}", 5, "foo", "jpg"),
            "foo_005"
        );
        assert_eq!(render_rename_template("img{n}", 12, "x", "png"), "img12");
        assert_eq!(
            render_rename_template("base.{ext}", 1, "a", "txt"),
            "base.txt"
        );
        // An unknown token is literal.
        assert_eq!(
            render_rename_template("a{bogus}b", 1, "x", "y"),
            "a{bogus}b"
        );
        // Multi-byte literals are preserved too.
        assert_eq!(render_rename_template("写真_{n}", 2, "x", "y"), "写真_2");
    }

    #[test]
    fn batch_rename_handles_swaps_via_two_phase() {
        let dir = unique_tmp("konoma_batch_rename_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        // a→c, b→a (a swap-like case where b becomes a while a still exists).
        let plan = vec![(a.clone(), dir.join("c.txt")), (b.clone(), a.clone())];
        batch_rename(&plan).unwrap();
        assert!(!b.exists(), "b は無くなる");
        assert_eq!(std::fs::read(dir.join("c.txt")).unwrap(), b"AAA", "c=元 a");
        assert_eq!(std::fs::read(&a).unwrap(), b"BBB", "a=元 b");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_rename_rolls_back_on_failure() {
        // Force an abort at the second loop's dst.exists() check, and confirm everything done up
        // to that point is restored to its original state.
        let dir = unique_tmp("konoma_batch_rename_rollback_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        // plan: a→c, b→d. But create d beforehand so it bails on the second loop's 2nd entry
        // (b→d). The 1st entry (a→c) will already be committed, so the rollback goes through both
        // the "committed dst→src" and "staged tmp→src" paths.
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        std::fs::write(&d, b"DDD").unwrap(); // rig the collision.
        let plan = vec![(a.clone(), c.clone()), (b.clone(), d.clone())];

        let res = batch_rename(&plan);
        assert!(res.is_err(), "リネーム先衝突で失敗すること");

        // Every src stays at its original name/content (rollback succeeded).
        assert_eq!(std::fs::read(&a).unwrap(), b"AAA", "a は原状復帰");
        assert_eq!(std::fs::read(&b).unwrap(), b"BBB", "b は原状復帰");
        // c, which was about to be committed, does not remain.
        assert!(!c.exists(), "c は巻き戻しで消える");
        // The pre-existing d created beforehand is untouched.
        assert_eq!(std::fs::read(&d).unwrap(), b"DDD", "既存 d は無傷");
        // No orphaned temp file (.konoma-rename-tmp-*) is left behind.
        let orphans: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".konoma-rename-tmp-"))
            .collect();
        assert!(
            orphans.is_empty(),
            "孤立した一時ファイルが残らない: {orphans:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_rename_rolls_back_swap_on_failure() {
        // The case where a subsequent entry fails in the second loop after a swap has already
        // been committed. Rolling back directly with dst→src would corrupt a surviving file by
        // overwriting it, since each swap element occupies the other's src (a regression). Confirm
        // the two-phase approach (dst→tmp→src) fully restores it.
        let dir = unique_tmp("konoma_batch_rename_swap_rollback_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let one = dir.join("1.txt");
        let two = dir.join("2.txt");
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        std::fs::write(&one, b"ONE").unwrap();
        std::fs::write(&two, b"TWO").unwrap();
        std::fs::write(&c, b"CCC").unwrap();
        std::fs::write(&d, b"DDD").unwrap(); // make the 3rd entry's dst pre-exist so the second loop bails.

        // plan: 2→1, 1→2 (a swap that commits) and c→d (fails because d already exists).
        let plan = vec![
            (two.clone(), one.clone()),
            (one.clone(), two.clone()),
            (c.clone(), d.clone()),
        ];
        let res = batch_rename(&plan);
        assert!(res.is_err(), "リネーム先衝突で失敗すること");

        // Even if it fails after the swap has been committed, it fully restores to 1=ONE /
        // 2=TWO / c=CCC (TWO is never lost).
        assert_eq!(std::fs::read(&one).unwrap(), b"ONE", "1.txt は原状復帰");
        assert_eq!(std::fs::read(&two).unwrap(), b"TWO", "2.txt は原状復帰");
        assert_eq!(std::fs::read(&c).unwrap(), b"CCC", "c.txt は原状復帰");
        // The pre-existing d created beforehand is untouched.
        assert_eq!(std::fs::read(&d).unwrap(), b"DDD", "既存 d は無傷");
        // No orphaned temp file is left behind.
        let orphans: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".konoma-rename-tmp-"))
            .collect();
        assert!(
            orphans.is_empty(),
            "孤立した一時ファイルが残らない: {orphans:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// When the rollback itself cannot finish, the leftovers have to be *reported*, not just
    /// logged: the user has to delete `.konoma-rename-tmp-*` by hand and has no way to know unless
    /// told. That news is attached as a typed `RollbackIncomplete` context precisely so the UI can
    /// find it with a downcast (a plain string context is invisible to `describe_file_op_error`,
    /// which is exactly how it used to get thrown away).
    ///
    /// `batch_rename`'s own two-phase rollback is robust enough that no *plan* can make it fail
    /// (the sibling tests above cover collisions and swaps), so the failure is injected at the
    /// level below: `rollback_batch_rename` is called directly with a staging state whose `src`
    /// slot has been taken over by a non-empty directory. `rename(file, non-empty dir)` fails for
    /// every user including root, so this needs no permission juggling and no root guard — and it
    /// is a real scenario, an agent creating a directory at the old name mid-rename.
    #[test]
    fn rollback_failure_reports_the_files_it_left_behind() {
        let dir = unique_tmp("konoma_rollback_incomplete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Entry 0: staged aside, and its original name is now occupied by a non-empty directory,
        // so restoring it is impossible.
        let stuck_tmp = dir.join(".konoma-rename-tmp-0");
        let stuck_src = dir.join("a.txt");
        let stuck_dst = dir.join("x.txt");
        std::fs::write(&stuck_tmp, b"AAA").unwrap();
        std::fs::create_dir_all(&stuck_src).unwrap();
        std::fs::write(stuck_src.join("blocker"), b"in the way").unwrap();

        // Entry 1: staged aside with nothing in its way, so it rolls back cleanly and must **not**
        // be reported as a leftover.
        let ok_tmp = dir.join(".konoma-rename-tmp-1");
        let ok_src = dir.join("b.txt");
        let ok_dst = dir.join("y.txt");
        std::fs::write(&ok_tmp, b"BBB").unwrap();

        let staged = vec![
            (stuck_tmp.clone(), stuck_src.clone(), stuck_dst),
            (ok_tmp.clone(), ok_src.clone(), ok_dst),
        ];
        let original = anyhow::Error::new(FileOpError::RenameCommitFailed(dir.join("boom")));
        let err = rollback_batch_rename(original, &staged, 0);

        // The reason the rename failed is still reachable (the leftover news is *added*, not
        // substituted).
        assert!(
            matches!(
                err.downcast_ref::<FileOpError>(),
                Some(FileOpError::RenameCommitFailed(_))
            ),
            "元の失敗理由が失われていない: {err:#}"
        );
        let rb = err
            .downcast_ref::<RollbackIncomplete>()
            .expect("巻き戻し失敗が型付きで伝わる");
        assert_eq!(
            rb.leftovers,
            vec![stuck_tmp.clone()],
            "戻せなかった一時ファイルだけを報告する"
        );
        // Ground truth on disk: the blocked one is still staged, the clean one is restored.
        assert!(stuck_tmp.is_file(), "戻せなかった一時ファイルは実在する");
        assert_eq!(std::fs::read(&ok_src).unwrap(), b"BBB", "b.txt は原状復帰");
        assert!(!ok_tmp.exists(), "戻せた方の一時ファイルは残らない");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Probe for the sibling test below: whether this process is actually stopped by a read-only
    /// directory. Running as root (a container, CI as root) it is not, and the test would go red
    /// for environmental reasons while the product is fine — the same situation
    /// `write_denied_by_permissions` in `app/tests.rs` exists for.
    #[cfg(unix)]
    fn rename_denied_by_directory_permissions() -> bool {
        use std::os::unix::fs::PermissionsExt;
        let dir = unique_tmp("konoma_ro_dir_probe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let denied = std::fs::rename(dir.join("f"), dir.join("g")).is_err();
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);
        denied
    }

    /// The other way an entry can be stranded: its rename was already **committed**, and unstaging
    /// it back fails, so the file is sitting under its *new* name rather than under a staging name.
    /// Reported as that new name, since that is where the user will find it.
    ///
    /// Needs a genuinely denied rename (a read-only directory), hence the root guard.
    #[cfg(unix)]
    #[test]
    fn rollback_failure_reports_a_committed_entry_under_its_new_name() {
        use std::os::unix::fs::PermissionsExt;
        if !rename_denied_by_directory_permissions() {
            eprintln!(
                "rollback_failure_reports_a_committed_entry_under_its_new_name: このプロセスはディレクトリのパーミッションで rename を拒否されない(root 等)ためスキップ"
            );
            return;
        }
        let dir = unique_tmp("konoma_rollback_committed_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The entry has already been committed: the content lives at `dst`, `tmp` and `src` are
        // both free. Freezing the directory makes both rollback phases fail.
        let tmp = dir.join(".konoma-rename-tmp-0");
        let src = dir.join("a.txt");
        let dst = dir.join("x.txt");
        std::fs::write(&dst, b"AAA").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let staged = vec![(tmp, src, dst.clone())];
        let original = anyhow::Error::new(FileOpError::RenameDestExists(dir.join("boom")));
        let err = rollback_batch_rename(original, &staged, 1);

        let rb = err
            .downcast_ref::<RollbackIncomplete>()
            .expect("巻き戻し失敗が型付きで伝わる");
        assert_eq!(
            rb.leftovers,
            vec![dst.clone()],
            "戻せなかった分は新しい名前で報告する"
        );
        assert!(
            matches!(
                err.downcast_ref::<FileOpError>(),
                Some(FileOpError::RenameDestExists(_))
            ),
            "元の失敗理由も残る: {err:#}"
        );

        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A rollback that fully succeeds must not claim anything was left behind (the pair to the test
    /// above — without this, "always attach RollbackIncomplete" would pass).
    #[test]
    fn successful_rollback_reports_nothing_left_behind() {
        let dir = unique_tmp("konoma_rollback_complete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        std::fs::write(&d, b"DDD").unwrap(); // rig the collision so the commit phase bails

        let err = batch_rename(&[(a.clone(), c), (b.clone(), d.clone())])
            .expect_err("既存 d.txt への衝突で失敗する");
        assert!(
            err.downcast_ref::<RollbackIncomplete>().is_none(),
            "巻き戻しが成功したなら残骸を主張しない: {err:#}"
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"AAA", "a は原状復帰");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_into_duplicates_and_recurses() {
        let dir = unique_tmp("konoma_copy_into_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"AAA").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // Copy into the same directory → "a copy.txt" (does not overwrite).
        let d = copy_into(&dir, &dir.join("a.txt")).unwrap();
        assert_eq!(d.file_name().unwrap(), "a copy.txt");
        assert_eq!(std::fs::read(&d).unwrap(), b"AAA");
        // Once more → "a copy 2.txt".
        let d2 = copy_into(&dir, &dir.join("a.txt")).unwrap();
        assert_eq!(d2.file_name().unwrap(), "a copy 2.txt");
        // Copying into a different directory keeps the same name.
        let d3 = copy_into(&sub, &dir.join("a.txt")).unwrap();
        assert_eq!(d3, sub.join("a.txt"));
        // Recursive directory copy.
        let tree = dir.join("tree");
        std::fs::create_dir_all(tree.join("inner")).unwrap();
        std::fs::write(tree.join("inner").join("x.txt"), b"x").unwrap();
        copy_into(&sub, &tree).unwrap();
        assert!(sub.join("tree").join("inner").join("x.txt").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_into_moves_and_noop_same_dir() {
        let dir = unique_tmp("konoma_move_into_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // Move to a different directory.
        let d = move_into(&sub, &dir.join("a.txt")).unwrap();
        assert!(d.is_file() && !dir.join("a.txt").exists());
        assert_eq!(d, sub.join("a.txt"));
        // Moving within the same directory = no-op.
        let back = move_into(&sub, &sub.join("a.txt")).unwrap();
        assert_eq!(back, sub.join("a.txt"));
        assert!(sub.join("a.txt").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_and_move_preserve_symlinks() {
        let dir = unique_tmp("konoma_symlink_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Real entries: a file and a directory.
        std::fs::write(dir.join("target.txt"), b"REAL").unwrap();
        let real_dir = dir.join("realdir");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("inner.txt"), b"in").unwrap();

        // symlink→file / symlink→dir
        let link_f = dir.join("link_f");
        let link_d = dir.join("link_d");
        std::os::unix::fs::symlink(dir.join("target.txt"), &link_f).unwrap();
        std::os::unix::fs::symlink(&real_dir, &link_d).unwrap();

        let into = dir.join("into");
        std::fs::create_dir_all(&into).unwrap();

        // Copying a symlink→file: keeps it a link (does not materialize it).
        let cf = copy_into(&into, &link_f).unwrap();
        assert!(
            std::fs::symlink_metadata(&cf)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink→file はコピー後も symlink"
        );
        assert_eq!(std::fs::read_link(&cf).unwrap(), dir.join("target.txt"));

        // Copying a symlink→dir: keeps it a link (does not follow its contents).
        let cd = copy_into(&into, &link_d).unwrap();
        assert!(
            std::fs::symlink_metadata(&cd)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink→dir はコピー後も symlink"
        );
        assert_eq!(std::fs::read_link(&cd).unwrap(), real_dir);

        // A dir-symlink inside a directory stays a symlink even through a recursive copy.
        let parent = dir.join("parent");
        std::fs::create_dir_all(&parent).unwrap();
        std::os::unix::fs::symlink(&real_dir, parent.join("nested_link")).unwrap();
        let cp = copy_into(&into, &parent).unwrap();
        assert!(
            std::fs::symlink_metadata(cp.join("nested_link"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "再帰コピー内の symlink も保たれる"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_info_and_formatters() {
        // Formatting helpers.
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1048576), "1.0 MB");
        assert_eq!(permission_string(0o755), "rwxr-xr-x (755)");
        assert_eq!(permission_string(0o100644), "rw-r--r-- (644)"); // low 9 bits even with the type bits included
        assert_eq!(format_epoch_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_epoch_utc(1577836800), "2020-01-01 00:00:00 UTC");
        assert_eq!(
            format_epoch_utc(1718323200 + 45296),
            "2024-06-14 12:34:56 UTC"
        );
        // The short form for column display, rwx, and cells.
        assert_eq!(format_epoch_short(1718323200 + 45296), "2024-06-14 12:34");
        assert_eq!(permission_rwx(0o100755), "rwxr-xr-x");
        assert_eq!(detail_column_width("size"), Some(9));
        assert_eq!(detail_column_width("type"), Some(4));
        assert_eq!(detail_column_width("items"), Some(6));
        assert_eq!(detail_column_width("bogus"), None);

        // Fetching a real file/directory.
        let dir = unique_tmp("konoma_fileinfo_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let fi = file_info(&dir.join("a.txt")).unwrap();
        assert!(!fi.is_dir && fi.size == 5 && fi.child_count.is_none());
        let di = file_info(&dir).unwrap();
        assert!(di.is_dir && di.child_count == Some(1));

        // detail_cell(RowMeta + path). Confirm size/type/items against the real thing.
        let fm = quick_meta(&dir.join("a.txt")).unwrap();
        assert_eq!(
            detail_cell("size", &dir.join("a.txt"), &fm).as_deref(),
            Some("5 B")
        );
        assert_eq!(
            detail_cell("type", &dir.join("a.txt"), &fm).as_deref(),
            Some("file")
        );
        let dm = quick_meta(&dir).unwrap();
        assert_eq!(detail_cell("size", &dir, &dm).as_deref(), Some("--"));
        assert_eq!(detail_cell("type", &dir, &dm).as_deref(), Some("dir"));
        assert_eq!(detail_cell("items", &dir, &dm).as_deref(), Some("1")); // the one entry, a.txt
        assert_eq!(detail_cell("bogus", &dir, &dm), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore] // not run normally since it dirties the real trash (run with cargo test -- --ignored)
    fn trash_moves_file_out_of_place() {
        let dir = unique_tmp("konoma_trash_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("trashme.txt");
        std::fs::write(&f, b"x").unwrap();
        move_to_trash(std::slice::from_ref(&f)).unwrap();
        assert!(!f.exists(), "ゴミ箱へ送られて元の場所から消える");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    // The real OS trash is environment-dependent on Linux (XDG dirs, cross-filesystem rules), so a
    // headless CI runner can't reliably exercise it. macOS (the primary target) always can. On Linux
    // the freedesktop path is used at runtime; only this live test is gated.
    #[cfg(target_os = "macos")]
    fn move_to_trash_removes_from_original_then_cleanup() {
        // Confirm it disappears from the original location, and best-effort clean up its trace
        // on the trash side.
        let dir = unique_tmp("konoma_trash_live_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = "konoma_trash_probe_5f3a9.txt"; // a unique name unlikely to collide
        let f = dir.join(name);
        std::fs::write(&f, b"trash me").unwrap();
        move_to_trash(std::slice::from_ref(&f)).unwrap();
        assert!(!f.exists(), "ゴミ箱送りで元の場所から消える");
        // Clean up macOS's trash (~/.Trash/<name>) so the real trash isn't left dirty. Any
        // rename-on-collision variant is cleaned up best-effort.
        if let Some(home) = std::env::var_os("HOME") {
            let _ = std::fs::remove_file(PathBuf::from(home).join(".Trash").join(name));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_file_errors_when_parent_is_a_file() {
        // If the path standing in for the parent is an existing file, create_dir_all fails →
        // error (does not crash).
        let dir = unique_tmp("konoma_create_file_parent_err_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blocker"), b"x").unwrap();
        let err = create_file(&dir, "blocker/inner.txt").unwrap_err();
        assert!(
            err.to_string().contains("create parent directory"),
            "親作成エラー: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_and_move_into_error_on_missing_source() {
        // A nonexistent src fails at symlink_metadata → Err (not swallowed).
        let dir = unique_tmp("konoma_copy_move_missing_src_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        let missing = dir.join("ghost.txt");
        assert!(
            copy_into(&dst, &missing).is_err(),
            "存在しない src のコピーは Err"
        );
        assert!(
            move_into(&dst, &missing).is_err(),
            "存在しない src の移動は Err"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn human_size_scales_through_large_units() {
        // The existing test only covers B/KB/MB. Also confirm GB/TB/PB and the PB clamp
        // (the top unit).
        assert_eq!(human_size(1024u64.pow(3)), "1.0 GB");
        assert_eq!(human_size(1024u64.pow(4)), "1.0 TB");
        assert_eq!(human_size(1024u64.pow(5)), "1.0 PB");
        // Stays at the top unit (PB) even beyond PB.
        assert_eq!(human_size(2 * 1024u64.pow(5)), "2.0 PB");
    }

    #[test]
    fn delete_permanently_reports_progress_per_path() {
        // Permanent deletion can loop per path, so progress advances one at a time (not all at
        // once at the end).
        // Sending to trash is a single call to `trash::delete_all`, so it can't do the same
        // (it jumps all at once when done).
        let dir = unique_tmp("konoma_progress_delete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for n in ["a.txt", "b.txt"] {
            let p = dir.join(n);
            std::fs::write(&p, b"x").unwrap();
            paths.push(p);
        }
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("c.txt"), b"c").unwrap();
        paths.push(sub);

        let p = Progress::default();
        delete_permanently_with_progress(&paths, &p).unwrap();
        assert_eq!(p.items(), 3, "3つのターゲットぶん items が進む");
        for path in &paths {
            assert!(!path.exists(), "対象が消えている: {}", path.display());
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `trash_partial_outcome` is the pure fs-observation step `App::run_file_op`'s `Trash` arm
    /// leans on when `move_to_trash` fails: it must count a target as "actually removed" purely
    /// by whether it's still there, in every combination (some gone / all present / all gone), and
    /// name the first still-present one as "something to act on" (or `None` when there's nothing
    /// left to report). Pure fs stat'ing, so no real Trash call and no permission tricks needed —
    /// this always runs, unlike the live-Trash test below.
    #[test]
    fn trash_partial_outcome_observes_which_targets_actually_disappeared() {
        let dir = unique_tmp("konoma_trash_partial_outcome_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gone = dir.join("gone.txt"); // never created: stands in for "the backend removed this one"
        let still_there = dir.join("still_there.txt");
        std::fs::write(&still_there, b"x").unwrap();

        let mixed = vec![gone.clone(), still_there.clone()];
        let (ok, remaining) = trash_partial_outcome(&mixed);
        assert_eq!(ok, 1, "gone の1件だけ実際に消えたと数える");
        assert_eq!(
            remaining,
            Some(&still_there),
            "まだ残っている対象を名指しする"
        );

        let all_present = vec![still_there.clone()];
        let (ok2, remaining2) = trash_partial_outcome(&all_present);
        assert_eq!(ok2, 0, "何も消えていなければ0=嘘の成功数を出さない");
        assert_eq!(remaining2, Some(&still_there));

        let all_gone = vec![gone.clone()];
        let (ok3, remaining3) = trash_partial_outcome(&all_gone);
        assert_eq!(ok3, 1, "全部消えていれば全数");
        assert_eq!(remaining3, None, "残っている対象が無ければ None");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Mirrors `write_denied_by_permissions` in `src/app/tests.rs` (same idea, duplicated rather
    /// than shared across the module boundary — this file already keeps its own `unique_tmp` for
    /// the same reason). Probes whether this process is actually denied a write, so permission-
    /// dependent tests can skip cleanly under root instead of going red for environmental reasons.
    #[cfg(unix)]
    fn write_denied_by_permissions() -> bool {
        let probe = unique_tmp("konoma_fileops_perm_probe");
        std::fs::write(&probe, "x").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o444)).unwrap();
        let denied = std::fs::OpenOptions::new()
            .write(true)
            .open(&probe)
            .is_err();
        let _ = std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_file(&probe);
        denied
    }

    /// `delete_permanently_with_progress` loops per path and returns via `?` on the first
    /// failure (see its doc comment) — this pins down that `Progress` only counts what was
    /// removed *before* the failing target, not 0 and not the full batch. This function itself
    /// was never the bug (it already bumped `p` correctly); `App::run_file_op`'s `DeletePermanent`
    /// arm discarding that count on error was (see
    /// `delete_permanent_partial_failure_reports_the_real_success_count` in `src/app/tests.rs` for
    /// the user-visible half of that fix) — so this test passes on both sides of that fix. It's
    /// kept here as a foundation check: the App-level fix reads `p.items()` back out, and this
    /// pins down that the primitive it reads actually behaves that way.
    #[test]
    #[cfg(unix)]
    fn delete_permanently_with_progress_counts_only_what_actually_got_removed() {
        if !write_denied_by_permissions() {
            eprintln!(
                "delete_permanently_with_progress_counts_only_what_actually_got_removed: このプロセスはパーミッションで書込みを拒否されない(root 等)ためスキップ"
            );
            return;
        }
        let dir = unique_tmp("konoma_progress_partial_delete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("inner.txt"), b"x").unwrap();
        std::fs::write(dir.join("c.txt"), b"c").unwrap();
        // Deny write on `blocked` itself: removing `inner.txt` (required before the directory
        // itself can go) needs write+exec on its containing directory, so this makes exactly the
        // *middle* target fail (in the order below) while its siblings stay removable.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let targets = vec![dir.join("a.txt"), blocked.clone(), dir.join("c.txt")];
        let p = Progress::default();
        let res = delete_permanently_with_progress(&targets, &p);

        // Restore permissions so cleanup below can actually remove `blocked`, regardless of outcome.
        let _ = std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755));

        assert!(res.is_err(), "blocked の削除失敗で Err になる");
        assert_eq!(
            p.items(),
            1,
            "a.txt の1件だけ実際に消えた分をProgressが数える(0でも3でもない)"
        );
        assert!(!dir.join("a.txt").exists(), "a.txt は実際に削除済み");
        assert!(dir.join("blocked").exists(), "blocked は失敗して残っている");
        assert!(
            dir.join("c.txt").exists(),
            "blocked で ? 早期returnしたので c.txt は未着手のまま残っている"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_op_progress_counts_leaf_files() {
        // Copy a whole tree containing nested subdirectories, and confirm Progress advances only
        // by the leaf-file count (directories themselves are not counted). The background file-op
        // worker's progress display ("N/M · files") depends on this counter.
        let dir = unique_tmp("konoma_progress_leaf_count_test");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("b.txt"), b"b").unwrap();
        std::fs::write(src.join("nested").join("c.txt"), b"c").unwrap();
        let dst = dir.join("dst");
        std::fs::create_dir_all(&dst).unwrap();

        let p = Progress::default();
        assert_eq!(p.files(), 0);
        let out = copy_into_with_progress(&dst, &src, &p).unwrap();
        assert!(out.join("a.txt").is_file());
        assert!(out.join("nested").join("c.txt").is_file());
        assert_eq!(
            p.files(),
            3,
            "3枚の末端ファイルぶん進む(ディレクトリは数えない)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `create_file`/`create_dir`/`rename` report a name collision as `FileOpError::AlreadyExists`
    /// (not just an ad-hoc string), so the UI layer (`App::describe_error`) can translate it instead
    /// of showing whatever language this module's own `Display` happens to be written in.
    #[test]
    fn already_exists_errors_downcast_to_file_op_error() {
        let dir = unique_tmp("konoma_fileops_typed_exists_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let e1 = create_file(&dir, "a.txt").unwrap_err();
        assert!(
            matches!(
                e1.downcast_ref::<FileOpError>(),
                Some(FileOpError::AlreadyExists(p)) if p == &dir.join("a.txt")
            ),
            "create_file: {e1:?}"
        );

        let e2 = create_dir(&dir, "sub").unwrap_err();
        assert!(
            matches!(
                e2.downcast_ref::<FileOpError>(),
                Some(FileOpError::AlreadyExists(p)) if p == &dir.join("sub")
            ),
            "create_dir: {e2:?}"
        );

        std::fs::write(dir.join("b.txt"), b"y").unwrap();
        let e3 = rename(&dir.join("b.txt"), "a.txt").unwrap_err();
        assert!(
            matches!(
                e3.downcast_ref::<FileOpError>(),
                Some(FileOpError::AlreadyExists(p)) if p == &dir.join("a.txt")
            ),
            "rename: {e3:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A source whose `file_name()` is unavailable (e.g. `/`) fails the copy/move with a typed
    /// `NameUnavailable`, before touching the filesystem any further.
    #[test]
    fn copy_into_with_progress_errors_typed_when_name_is_unavailable() {
        let dir = unique_tmp("konoma_fileops_name_unavailable_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let p = Progress::default();
        let err = copy_into_with_progress(&dir, Path::new("/"), &p).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<FileOpError>(),
                Some(FileOpError::NameUnavailable(src)) if src == Path::new("/")
            ),
            "{err:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `batch_rename` itself re-checks the destination for a collision (defense in depth beyond
    /// `App::build_rename_plan`'s own pre-check) and reports it as a typed `RenameDestExists`,
    /// distinct from the single-file `AlreadyExists` (it's the batch apply phase, not create/rename).
    #[test]
    fn batch_rename_dest_exists_downcasts_to_file_op_error() {
        let dir = unique_tmp("konoma_fileops_batch_dest_exists_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let existing = dir.join("existing.txt");
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&existing, b"E").unwrap();

        // Call fileops::batch_rename directly (bypassing App::build_rename_plan's own collision
        // pre-check) with a plan whose destination already exists on disk.
        let err = batch_rename(&[(a, existing.clone())]).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<FileOpError>(),
                Some(FileOpError::RenameDestExists(p)) if p == &existing
            ),
            "{err:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
