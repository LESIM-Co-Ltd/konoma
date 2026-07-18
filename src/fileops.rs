//! File operation primitives (M7 Phase B). Destructive operations default to **a confirmation dialog plus moving to Trash** (safety first).
//! This module handles only fs operations; the confirmation UI and key handling live on the app/ui side.
//! The principles are: never overwrite existing entries (create/rename error on collision) and keep deletion recoverable (Trash).

use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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
        std::fs::symlink_metadata(path).with_context(|| format!("情報取得: {}", path.display()))?;
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
    // civil_from_days: 1970-01-01 からの日数 → 年月日。
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

/// Move multiple paths to Trash (recoverable). On macOS this is the proper Trash (supports Finder's "Put Back").
pub fn move_to_trash(paths: &[PathBuf]) -> Result<()> {
    trash::delete_all(paths).context("ゴミ箱への移動に失敗しました")?;
    Ok(())
}

/// **Permanently delete** multiple paths (unrecoverable; does not go through Trash). Directories are deleted with their contents.
/// For a symlink, only the link itself is removed (the target is not followed).
pub fn delete_permanently(paths: &[PathBuf]) -> Result<()> {
    for p in paths {
        let meta =
            std::fs::symlink_metadata(p).with_context(|| format!("情報取得: {}", p.display()))?;
        if meta.is_dir() {
            std::fs::remove_dir_all(p)
                .with_context(|| format!("ディレクトリ完全削除: {}", p.display()))?;
        } else {
            std::fs::remove_file(p).with_context(|| format!("完全削除: {}", p.display()))?;
        }
    }
    Ok(())
}

/// Create an empty file directly under `dir` (or in a hierarchy included in `name`). Errors if it already exists (does not overwrite).
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        anyhow::bail!("既に存在します: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("親ディレクトリ作成: {}", parent.display()))?;
    }
    std::fs::File::create(&path).with_context(|| format!("ファイル作成: {}", path.display()))?;
    Ok(path)
}

/// Create a directory directly under `dir` (also creating intermediate directories). Errors if it already exists.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        anyhow::bail!("既に存在します: {}", path.display());
    }
    std::fs::create_dir_all(&path)
        .with_context(|| format!("ディレクトリ作成: {}", path.display()))?;
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
        anyhow::bail!("既に存在します: {}", dst.display());
    }
    std::fs::rename(src, &dst).with_context(|| format!("リネーム: {}", dst.display()))?;
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
        // `{` は ASCII(0x7B)。UTF-8 の多バイト文字の途中に現れないので byte 判定で安全。
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
        // `{n:0W}` = 幅 W のゼロ埋め連番。
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
    // 第1ループで退避したもの: (一時名 tmp, 元src, 最終dst)。
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
    // 第2ループで確定した件数(staged の先頭から数える)。`staged[..committed]` が確定済み。
    let mut committed = 0usize;

    for (i, (src, dst)) in plan.iter().enumerate() {
        if src == dst {
            continue;
        }
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(".konoma-rename-tmp-{i}"));
        if tmp.exists() {
            let err = anyhow::anyhow!("一時ファイル名が既存: {}", tmp.display());
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        if let Err(e) = std::fs::rename(src, &tmp) {
            let err =
                anyhow::Error::new(e).context(format!("一括リネーム(一時退避): {}", src.display()));
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        staged.push((tmp, src.clone(), dst.clone()));
    }

    for idx in 0..staged.len() {
        let (tmp, _src, dst) = &staged[idx];
        if dst.exists() {
            let err = anyhow::anyhow!("リネーム先が既存: {}", dst.display());
            return Err(rollback_batch_rename(err, &staged, committed));
        }
        if let Err(e) = std::fs::rename(tmp, dst) {
            let err =
                anyhow::Error::new(e).context(format!("一括リネーム(確定): {}", dst.display()));
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
    // 1) 確定済みを dst → tmp へ戻す。tmp 名(`.konoma-rename-tmp-{i}`)は未使用なので衝突しない。
    //    これで未確定分(既に tmp)と合わせ全 staged が tmp に揃い、入れ替え/巡回でも src 同士の
    //    上書き衝突が起きなくなる。逆順に処理して前進の確定順を巻き戻す。
    for (tmp, _src, dst) in staged[..committed].iter().rev() {
        if let Err(e) = std::fs::rename(dst, tmp) {
            err = err.context(format!(
                "巻き戻し失敗(確定分退避 {} → {}): {e}",
                dst.display(),
                tmp.display()
            ));
        }
    }
    // 2) 全 staged を tmp → src へ戻す。全 src は空いているので入れ替え/巡回でも衝突しない。
    for (tmp, src, _dst) in staged.iter().rev() {
        if let Err(e) = std::fs::rename(tmp, src) {
            err = err.context(format!(
                "巻き戻し失敗(復元 {} → {}): {e}",
                tmp.display(),
                src.display()
            ));
        }
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
        std::fs::read_link(src).with_context(|| format!("リンク読込: {}", src.display()))?;
    std::os::unix::fs::symlink(&target, dst)
        .with_context(|| format!("リンク作成: {}", dst.display()))?;
    Ok(())
}

/// Recursively copy a directory (done by hand since `std::fs` lacks it). `dst` is newly created.
/// Symlinks are duplicated as links and their targets are not followed (to prevent accidental materialization/following).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    // 自己包含ガード: `dst` が `src` の部分木内だと read_dir が増殖中の `dst` を拾い続け
    // 無限再帰する(例: Finder から親フォルダを子へ D&D)。paste() の starts_with ガードと対称に、
    // コピー/移動/ドロップの全経路をここで一括防御する。
    if dst.starts_with(src) {
        anyhow::bail!(
            "コピー先がコピー元の配下です: {} ⊂ {}",
            dst.display(),
            src.display()
        );
    }
    std::fs::create_dir_all(dst).with_context(|| format!("作成: {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("読込: {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            copy_symlink(&from, &to)?;
        } else if ft.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("コピー: {}", from.display()))?;
        }
    }
    Ok(())
}

/// **Copy** `src` (file/directory) directly under `dir`. On collision, append ` copy` and **do not overwrite**.
/// Copying into the same directory produces a duplicate (`name copy`). Returns the resolved destination path.
pub fn copy_into(dir: &Path, src: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context("名前の取得に失敗")?;
    let dst = dir.join(unique_name(dir, name));
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, &dst)?;
    } else if meta.is_dir() {
        copy_dir_all(src, &dst)?;
    } else {
        std::fs::copy(src, &dst).with_context(|| format!("コピー: {}", src.display()))?;
    }
    Ok(dst)
}

/// **Move** `src` directly under `dir`. Moving into the same directory is a no-op. On collision, append ` copy` and do not overwrite.
/// If rename is impossible (e.g. across volumes), fall back to copy plus deleting the original. Returns the resolved destination path.
pub fn move_into(dir: &Path, src: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context("名前の取得に失敗")?;
    let same = dir.join(name);
    if same == src {
        return Ok(same); // 既に同じ場所
    }
    let dst = dir.join(unique_name(dir, name));
    if std::fs::rename(src, &dst).is_ok() {
        return Ok(dst);
    }
    // 別ボリューム等 → コピーして元を削除。symlink はリンク自体を複製し参照先を辿らない。
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, &dst)?;
        std::fs::remove_file(src)?;
    } else if meta.is_dir() {
        copy_dir_all(src, &dst)?;
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::copy(src, &dst).with_context(|| format!("移動(コピー): {}", src.display()))?;
        std::fs::remove_file(src)?;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_into_self_subtree_is_rejected_not_infinite() {
        // 親フォルダをその子へコピー(Finder ドロップ等)。自己包含ガードが無いと
        // read_dir が増殖中のコピー先を拾い続け `/parent/child/parent/child/…` を無限生成する。
        // ガードにより「コピー先がコピー元の配下」で即エラーになることを確認する(D1)。
        let root = std::env::temp_dir().join("konoma_selfcopy_test");
        let _ = std::fs::remove_dir_all(&root);
        let parent = root.join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(parent.join("f.txt"), b"x").unwrap();

        // parent を child の中へコピー → 自己包含なので Err(無限再帰しない)。
        let err = copy_into(&child, &parent).unwrap_err();
        assert!(
            err.to_string().contains("配下"),
            "自己包含エラーであるべき: {err}"
        );
        // 無限ネストが作られていない(child 配下に parent が生えていない)。
        assert!(
            !child.join("parent").join("parent").exists(),
            "増殖したネストが残ってはならない"
        );

        // move_into も同じガードを通る(rename 失敗→copy_dir_all 経路)。ただし rename が
        // 成功してしまう環境では move されるだけなので、ここでは copy 経路のみを検証対象とする。

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_and_rename_respect_existing() {
        let dir = std::env::temp_dir().join("konoma_fileops_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 作成。
        let f = create_file(&dir, "a.txt").unwrap();
        assert!(f.is_file());
        // 既存は上書きせずエラー。
        assert!(
            create_file(&dir, "a.txt").is_err(),
            "既存ファイルは上書きしない"
        );
        // 階層付き作成 (sub/b.txt)。
        let nested = create_file(&dir, "sub/b.txt").unwrap();
        assert!(nested.is_file());
        // ディレクトリ作成。
        let d = create_dir(&dir, "newdir").unwrap();
        assert!(d.is_dir());
        assert!(
            create_dir(&dir, "newdir").is_err(),
            "既存ディレクトリはエラー"
        );

        // リネーム。
        let renamed = rename(&f, "a2.txt").unwrap();
        assert!(renamed.is_file() && !f.exists());
        assert_eq!(renamed.file_name().unwrap(), "a2.txt");
        // 同名は no-op で成功。
        assert!(rename(&renamed, "a2.txt").is_ok());
        // 既存名へのリネームはエラー(上書きしない)。
        create_file(&dir, "c.txt").unwrap();
        assert!(rename(&renamed, "c.txt").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_permanently_removes_file_and_dir() {
        let dir = std::env::temp_dir().join("konoma_perm_delete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // ファイルとディレクトリ(中身あり)を完全削除。
        let f = dir.join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"y").unwrap();

        delete_permanently(&[f.clone(), sub.clone()]).unwrap();
        assert!(!f.exists(), "ファイルが完全削除される");
        assert!(!sub.exists(), "ディレクトリが中身ごと完全削除される");
        // 存在しないパスはエラー。
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
        // 未知トークンはリテラル。
        assert_eq!(
            render_rename_template("a{bogus}b", 1, "x", "y"),
            "a{bogus}b"
        );
        // マルチバイトのリテラルも保持。
        assert_eq!(render_rename_template("写真_{n}", 2, "x", "y"), "写真_2");
    }

    #[test]
    fn batch_rename_handles_swaps_via_two_phase() {
        let dir = std::env::temp_dir().join("konoma_batch_rename_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        // a→c, b→a (a が既存のまま b を a にする入れ替え系)。
        let plan = vec![(a.clone(), dir.join("c.txt")), (b.clone(), a.clone())];
        batch_rename(&plan).unwrap();
        assert!(!b.exists(), "b は無くなる");
        assert_eq!(std::fs::read(dir.join("c.txt")).unwrap(), b"AAA", "c=元 a");
        assert_eq!(std::fs::read(&a).unwrap(), b"BBB", "a=元 b");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_rename_rolls_back_on_failure() {
        // 第2ループの dst.exists() で中断させ、それまでの操作が原状回復されることを確認する。
        let dir = std::env::temp_dir().join("konoma_batch_rename_rollback_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        // plan: a→c, b→d。だが d を事前に作っておき、第2ループの 2 件目 (b→d) で bail させる。
        // 1 件目 (a→c) は確定済みになるので、巻き戻しは「確定分 dst→src」と
        // 「退避分 tmp→src」の両経路を通る。
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        std::fs::write(&d, b"DDD").unwrap(); // 衝突を仕込む。
        let plan = vec![(a.clone(), c.clone()), (b.clone(), d.clone())];

        let res = batch_rename(&plan);
        assert!(res.is_err(), "リネーム先衝突で失敗すること");

        // 全 src が元の名前・内容のまま(巻き戻し成功)。
        assert_eq!(std::fs::read(&a).unwrap(), b"AAA", "a は原状復帰");
        assert_eq!(std::fs::read(&b).unwrap(), b"BBB", "b は原状復帰");
        // 確定しかけた c は残らない。
        assert!(!c.exists(), "c は巻き戻しで消える");
        // 事前に作った既存 d は無傷。
        assert_eq!(std::fs::read(&d).unwrap(), b"DDD", "既存 d は無傷");
        // 孤立した一時ファイル(.konoma-rename-tmp-*)が残らない。
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
        // 確定済みの入れ替え(swap)後に後続が第2ループで失敗するケース。
        // 直接 dst→src で巻き戻すと swap の各要素が互いの src を占有しており生存ファイルを
        // 上書き破壊する(回帰)。二段階(dst→tmp→src)で完全復元することを確認する。
        let dir = std::env::temp_dir().join("konoma_batch_rename_swap_rollback_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let one = dir.join("1.txt");
        let two = dir.join("2.txt");
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        std::fs::write(&one, b"ONE").unwrap();
        std::fs::write(&two, b"TWO").unwrap();
        std::fs::write(&c, b"CCC").unwrap();
        std::fs::write(&d, b"DDD").unwrap(); // 3 件目の dst を既存にして第2ループで bail させる。

        // plan: 2→1, 1→2(swap・確定する) と c→d(d 既存で失敗)。
        let plan = vec![
            (two.clone(), one.clone()),
            (one.clone(), two.clone()),
            (c.clone(), d.clone()),
        ];
        let res = batch_rename(&plan);
        assert!(res.is_err(), "リネーム先衝突で失敗すること");

        // swap が確定後に失敗しても 1=ONE / 2=TWO / c=CCC に完全復元する(TWO 消失が起きない)。
        assert_eq!(std::fs::read(&one).unwrap(), b"ONE", "1.txt は原状復帰");
        assert_eq!(std::fs::read(&two).unwrap(), b"TWO", "2.txt は原状復帰");
        assert_eq!(std::fs::read(&c).unwrap(), b"CCC", "c.txt は原状復帰");
        // 事前に作った既存 d は無傷。
        assert_eq!(std::fs::read(&d).unwrap(), b"DDD", "既存 d は無傷");
        // 孤立した一時ファイルが残らない。
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
    fn copy_into_duplicates_and_recurses() {
        let dir = std::env::temp_dir().join("konoma_copy_into_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"AAA").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // 同ディレクトリへコピー → "a copy.txt"(上書きしない)。
        let d = copy_into(&dir, &dir.join("a.txt")).unwrap();
        assert_eq!(d.file_name().unwrap(), "a copy.txt");
        assert_eq!(std::fs::read(&d).unwrap(), b"AAA");
        // もう一度 → "a copy 2.txt"。
        let d2 = copy_into(&dir, &dir.join("a.txt")).unwrap();
        assert_eq!(d2.file_name().unwrap(), "a copy 2.txt");
        // 別ディレクトリへは同名でコピー。
        let d3 = copy_into(&sub, &dir.join("a.txt")).unwrap();
        assert_eq!(d3, sub.join("a.txt"));
        // ディレクトリの再帰コピー。
        let tree = dir.join("tree");
        std::fs::create_dir_all(tree.join("inner")).unwrap();
        std::fs::write(tree.join("inner").join("x.txt"), b"x").unwrap();
        copy_into(&sub, &tree).unwrap();
        assert!(sub.join("tree").join("inner").join("x.txt").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_into_moves_and_noop_same_dir() {
        let dir = std::env::temp_dir().join("konoma_move_into_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // 別ディレクトリへ移動。
        let d = move_into(&sub, &dir.join("a.txt")).unwrap();
        assert!(d.is_file() && !dir.join("a.txt").exists());
        assert_eq!(d, sub.join("a.txt"));
        // 同ディレクトリへの移動 = no-op。
        let back = move_into(&sub, &sub.join("a.txt")).unwrap();
        assert_eq!(back, sub.join("a.txt"));
        assert!(sub.join("a.txt").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_and_move_preserve_symlinks() {
        let dir = std::env::temp_dir().join("konoma_symlink_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 実体: ファイルとディレクトリ。
        std::fs::write(dir.join("target.txt"), b"REAL").unwrap();
        let real_dir = dir.join("realdir");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("inner.txt"), b"in").unwrap();

        // symlink→file / symlink→dir。
        let link_f = dir.join("link_f");
        let link_d = dir.join("link_d");
        std::os::unix::fs::symlink(dir.join("target.txt"), &link_f).unwrap();
        std::os::unix::fs::symlink(&real_dir, &link_d).unwrap();

        let into = dir.join("into");
        std::fs::create_dir_all(&into).unwrap();

        // symlink→file のコピー: リンクを保つ(実体化しない)。
        let cf = copy_into(&into, &link_f).unwrap();
        assert!(
            std::fs::symlink_metadata(&cf)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink→file はコピー後も symlink"
        );
        assert_eq!(std::fs::read_link(&cf).unwrap(), dir.join("target.txt"));

        // symlink→dir のコピー: リンクを保つ(中身を辿らない)。
        let cd = copy_into(&into, &link_d).unwrap();
        assert!(
            std::fs::symlink_metadata(&cd)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink→dir はコピー後も symlink"
        );
        assert_eq!(std::fs::read_link(&cd).unwrap(), real_dir);

        // ディレクトリ内部の dir-symlink を再帰コピーしても symlink を保つ。
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
        // 整形ヘルパ。
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1048576), "1.0 MB");
        assert_eq!(permission_string(0o755), "rwxr-xr-x (755)");
        assert_eq!(permission_string(0o100644), "rw-r--r-- (644)"); // 種別ビット込みでも下位9bit
        assert_eq!(format_epoch_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_epoch_utc(1577836800), "2020-01-01 00:00:00 UTC");
        assert_eq!(
            format_epoch_utc(1718323200 + 45296),
            "2024-06-14 12:34:56 UTC"
        );
        // 列表示用の短い形式・rwx・セル。
        assert_eq!(format_epoch_short(1718323200 + 45296), "2024-06-14 12:34");
        assert_eq!(permission_rwx(0o100755), "rwxr-xr-x");
        assert_eq!(detail_column_width("size"), Some(9));
        assert_eq!(detail_column_width("type"), Some(4));
        assert_eq!(detail_column_width("items"), Some(6));
        assert_eq!(detail_column_width("bogus"), None);

        // 実ファイル/ディレクトリの取得。
        let dir = std::env::temp_dir().join("konoma_fileinfo_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let fi = file_info(&dir.join("a.txt")).unwrap();
        assert!(!fi.is_dir && fi.size == 5 && fi.child_count.is_none());
        let di = file_info(&dir).unwrap();
        assert!(di.is_dir && di.child_count == Some(1));

        // detail_cell(RowMeta + path)。size/type/items を実物で確認。
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
        assert_eq!(detail_cell("items", &dir, &dm).as_deref(), Some("1")); // a.txt の1件
        assert_eq!(detail_cell("bogus", &dir, &dm), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore] // 実ゴミ箱を汚すため通常は走らせない (cargo test -- --ignored で実行)
    fn trash_moves_file_out_of_place() {
        let dir = std::env::temp_dir().join("konoma_trash_test");
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
        // 元の場所から消えることを確認し、ゴミ箱側の痕跡はベストエフォートで掃除する。
        let dir = std::env::temp_dir().join("konoma_trash_live_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = "konoma_trash_probe_5f3a9.txt"; // 衝突しにくいユニーク名
        let f = dir.join(name);
        std::fs::write(&f, b"trash me").unwrap();
        move_to_trash(std::slice::from_ref(&f)).unwrap();
        assert!(!f.exists(), "ゴミ箱送りで元の場所から消える");
        // macOS のゴミ箱(~/.Trash/<name>)を掃除(実ゴミ箱を汚さない)。衝突時の改名分は best-effort。
        if let Some(home) = std::env::var_os("HOME") {
            let _ = std::fs::remove_file(PathBuf::from(home).join(".Trash").join(name));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_file_errors_when_parent_is_a_file() {
        // 親に当たるパスが既存ファイルだと create_dir_all が失敗 → エラー(クラッシュしない)。
        let dir = std::env::temp_dir().join("konoma_create_file_parent_err_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blocker"), b"x").unwrap();
        let err = create_file(&dir, "blocker/inner.txt").unwrap_err();
        assert!(
            err.to_string().contains("親ディレクトリ作成"),
            "親作成エラー: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_and_move_into_error_on_missing_source() {
        // 存在しない src は symlink_metadata で失敗 → Err(握りつぶさない)。
        let dir = std::env::temp_dir().join("konoma_copy_move_missing_src_test");
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
        // 既存テストは B/KB/MB のみ。GB/TB/PB と PB クランプ(最大単位)も確認する。
        assert_eq!(human_size(1024u64.pow(3)), "1.0 GB");
        assert_eq!(human_size(1024u64.pow(4)), "1.0 TB");
        assert_eq!(human_size(1024u64.pow(5)), "1.0 PB");
        // PB を超えても最上位単位(PB)で留まる。
        assert_eq!(human_size(2 * 1024u64.pow(5)), "2.0 PB");
    }
}
