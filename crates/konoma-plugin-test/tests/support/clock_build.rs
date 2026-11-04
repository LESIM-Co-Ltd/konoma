// Build the real clock with the caller's target directory unchanged.
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub fn build(
    cargo: &OsStr,
    manifest_dir: &Path,
    test_exe: &Path,
    target_root: &Path,
    run: impl FnOnce(&mut Command) -> io::Result<ExitStatus>,
) -> io::Result<PathBuf> {
    let profile_dir = test_exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .canonicalize()?;
    let target_root = target_root.canonicalize()?;
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--offline",
            "-p",
            "konoma-demo-clock",
            "--bin",
            "konoma-demo-clock",
        ])
        .current_dir(manifest_dir);
    let profile = profile_dir
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    command.arg("--profile").arg(if profile == "debug" {
        OsStr::new("dev")
    } else {
        profile
    });
    let parent = profile_dir
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let target = parent
        .strip_prefix(&target_root)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if !target.as_os_str().is_empty() {
        if target.components().count() != 1 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        command.arg("--target").arg(target);
    }
    if !run(&mut command)?.success() {
        return Err(io::Error::other("clock build failed"));
    }
    Ok(profile_dir.join(format!("konoma-demo-clock{}", std::env::consts::EXE_SUFFIX)))
}
