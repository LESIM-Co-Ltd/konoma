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
    let target_root = target_root.canonicalize()?;
    // The test executable sits in `<target>/[<triple>/]<profile>/deps/` or, with the newer build-dir
    // layout, in `<profile>/build/<pkg>/<hash>/out/`: the profile directory is the ancestor that is
    // one level (or, for a cross build, two) under the target root.
    let exe_dir = test_exe
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .canonicalize()?;
    let (profile_dir, parent) = exe_dir
        .ancestors()
        .find_map(|a| {
            let parent = a.parent()?;
            let one = parent == target_root;
            let two = parent.parent() == Some(target_root.as_path());
            // A profile directory holds `deps/` (or `build/`); a target-triple directory does not.
            let is_profile = a.join("deps").is_dir() || a.join("build").is_dir();
            (is_profile && (one || two)).then(|| (a.to_path_buf(), parent.to_path_buf()))
        })
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
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

pub fn clock_build_checks_the_new_binary_for_release_and_cross_profiles() {
    use crate::command::CommandOutput;
    let dir = test_support::unique_tmp("clock-build-profile");
    std::fs::create_dir_all(&dir).unwrap();
    let cargo = dir.join("cargo");
    test_support::write_executable(
        &cargo,
        r#"#!/bin/sh
profile=dev
target=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile) shift; profile=$1 ;;
    --target) shift; target=$1 ;;
  esac
  shift
done
if [ "$profile" = dev ]; then profile=debug; fi
dest="$FAKE_TARGET/$target/$profile"
mkdir -p "$dest"
printf '#!/bin/sh\nprintf fresh' > "$dest/konoma-demo-clock"
chmod +x "$dest/konoma-demo-clock"
"#,
    );
    let target_root = dir.join("target");
    for (profile, target) in [("release", ""), ("ci", "aarch64-unknown-linux-gnu")] {
        let profile_dir = target_root.join(target).join(profile);
        // The harness is in `deps/`, or in `build/<pkg>/<hash>/out/` with the newer layout.
        for layout in ["deps", "build/konoma-plugin-test/0123abcd/out"] {
            let exe_dir = profile_dir.join(layout);
            std::fs::create_dir_all(&exe_dir).unwrap();
            test_support::write_executable(
                &profile_dir.join("konoma-demo-clock"),
                "#!/bin/sh\nprintf stale\n",
            );
            let executable = build(
                cargo.as_os_str(),
                &dir,
                &exe_dir.join("protocol"),
                &target_root,
                |command| {
                    command
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin")
                        .env("FAKE_TARGET", &target_root)
                        .supervised_status()
                },
            )
            .unwrap();
            let output = std::process::Command::new(executable)
                .supervised_output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                output.stdout, b"fresh",
                "profile {profile}, target {target}, layout {layout}"
            );
        }
    }
}

mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    pub struct TmpDir(PathBuf);
    impl AsRef<Path> for TmpDir {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }
    impl std::ops::Deref for TmpDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn unique_tmp(label: &str) -> TmpDir {
        loop {
            let path = std::env::temp_dir().join(format!(
                "konoma-checker-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return TmpDir(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create private fixture directory: {error}"),
            }
        }
    }
    pub fn write_executable(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
