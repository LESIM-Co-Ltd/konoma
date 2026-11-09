//! Pure file-set ordering and cyclic selection. Hosts decide eligibility (including existence).
use std::path::{Path, PathBuf};

pub fn ordered_changed_paths(
    root: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut paths: Vec<_> = paths.into_iter().filter(|p| p.starts_with(root)).collect();
    paths.sort();
    paths.dedup();
    paths
}

pub fn step_changed_path(paths: &[PathBuf], current: &Path, dir: i64) -> Option<PathBuf> {
    let len = paths.len() as i64;
    if len == 0 {
        return None;
    }
    let index = match paths.iter().position(|p| p == current) {
        Some(i) => (i as i64 + dir).rem_euclid(len) as usize,
        None if dir > 0 => 0,
        None => (len - 1) as usize,
    };
    Some(paths[index].clone())
}

pub fn changed_path_position(paths: &[PathBuf], current: &Path) -> Option<(usize, usize)> {
    Some((paths.iter().position(|p| p == current)? + 1, paths.len()))
}
