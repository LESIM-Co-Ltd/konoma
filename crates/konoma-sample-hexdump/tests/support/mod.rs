use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod command {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/command.rs"
    ));
}

pub fn target_root() -> PathBuf {
    std::path::absolute(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")),
    )
    .unwrap()
    .canonicalize()
    .unwrap()
}

// Match the running test's profile and target without changing its CARGO_TARGET_DIR.
pub fn cargo() -> (Command, PathBuf) {
    let exe = std::env::current_exe().unwrap();
    let root = target_root();
    // The test executable is in `<profile>/deps/` or, in the newer build-dir layout, in
    // `<profile>/build/<pkg>/<hash>/out/`: find the profile directory among its ancestors.
    let (profile_dir, parent) = exe
        .canonicalize()
        .unwrap()
        .ancestors()
        .skip(1)
        .find_map(|a| {
            let parent = a.parent()?;
            let ok = parent == root || parent.parent() == Some(root.as_path());
            (ok && (a.join("deps").is_dir() || a.join("build").is_dir()))
                .then(|| (a.to_path_buf(), parent.to_path_buf()))
        })
        .expect("a profile directory above the test executable");
    let mut command = Command::new(env!("CARGO"));
    command.args(["build", "--offline", "--profile"]);
    let profile = profile_dir.file_name().unwrap();
    command.arg(if profile == "debug" {
        OsStr::new("dev")
    } else {
        profile
    });
    let target = parent.strip_prefix(&root).unwrap();
    if !target.as_os_str().is_empty() {
        assert_eq!(target.components().count(), 1);
        command.arg("--target").arg(target);
    }
    (command, profile_dir)
}
