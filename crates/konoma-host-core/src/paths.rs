//! How a path is shown in a title: relative to the directory the program was opened in, or
//! `~/...`.

use std::path::{Path, PathBuf};

/// A relative display string based on `open_dir` (the launch directory). For paths underneath, it puts the launch directory name first (e.g. `A/sub/x`),
/// and for things outside open_dir (siblings/ancestors) it relativizes with `..` (e.g. `../B/aaa.md`, `..`). If it cannot be relativized, `~/...`.
/// A shared function to guarantee the **same base** for the top-left path display (`format_path`'s Relative) and path copy (`cr`/`yr`).
pub fn rel_to_open(open_dir: &Path, path: &Path) -> String {
    if path.starts_with(open_dir) {
        // Under open_dir: relativize against its parent so the launch directory name comes first (e.g. A/sub/x).
        let base = open_dir.parent().unwrap_or(open_dir);
        match path.strip_prefix(base) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    } else {
        // Outside open_dir (siblings/ancestors, etc.): show a path relative to open_dir, including `..` (e.g. ../B/aaa.md).
        // Without this, a file in sibling B would look like `B/aaa.md` and be mistaken for something underneath.
        match rel_from(open_dir, path) {
            Some(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    }
}

/// `~/...` if under HOME, otherwise the full path.
pub fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(rel) = path.strip_prefix(&home) {
            if rel.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Compute the relative path from `base` to `target`, including `..` (equivalent to pathdiff). It removes the common leading components and
/// pushes one `..` for each remaining level on the base side. None if the root kinds (absolute/drive) disagree and it cannot be relativized.
pub fn rel_from(base: &Path, target: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let bc: Vec<Component> = base.components().collect();
    let tc: Vec<Component> = target.components().collect();
    let mut i = 0;
    while i < bc.len() && i < tc.len() && bc[i] == tc[i] {
        i += 1;
    }
    // No common component, and one side starts at the root (absolute/Prefix) = cannot be relativized.
    if i == 0 {
        let is_root = |c: Option<&Component>| {
            matches!(c, Some(Component::Prefix(_)) | Some(Component::RootDir))
        };
        if is_root(bc.first()) || is_root(tc.first()) {
            return None;
        }
    }
    let mut rel = PathBuf::new();
    for _ in i..bc.len() {
        rel.push("..");
    }
    for c in &tc[i..] {
        rel.push(c.as_os_str());
    }
    Some(rel)
}
