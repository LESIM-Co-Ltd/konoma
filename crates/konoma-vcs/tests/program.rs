//! The project program refuses invalid directories before checking the terminal. Protocol modes
//! expose only Git and Jujutsu; standard file plugins are Preview destinations only.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

use konoma_test_support::{
    run_program_without_terminal, run_program_without_terminal_with_cwd, unique_tmp, TmpDir,
};

const EXE: &str = env!("CARGO_BIN_EXE_konoma-vcs");
const TERMINAL: &str = "standard input and output must be a terminal";
const BACKEND: &str = "backend is unavailable or cannot open this repository";

fn directory() -> TmpDir {
    let dir = unique_tmp("vcs_program");
    std::fs::create_dir_all(&dir).unwrap();
    dir.into_canonical().unwrap()
}

fn git_available() -> bool {
    if Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        true
    } else {
        eprintln!("SKIP: git is not on PATH; repository program case was not run");
        false
    }
}

fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git init: {out:?}");
}

fn run(home: &Path, args: &[&OsStr]) -> Output {
    run_program_without_terminal(Path::new(EXE), args, home)
}

fn assert_reason(out: &Output, code: i32, reason: &str) -> String {
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(code), "{err}");
    assert!(err.contains(reason), "expected {reason:?}: {err}");
    err
}

fn assert_repository(out: &Output) {
    if cfg!(feature = "git") {
        assert_reason(out, 1, TERMINAL);
    } else {
        let err = assert_reason(out, 1, BACKEND);
        assert!(err.contains("cannot open the project"), "{err}");
        assert!(!err.contains(TERMINAL), "{err}");
    }
}

#[test]
fn no_argument_opens_the_repository_in_cwd() {
    if !git_available() {
        return;
    }
    let home = directory();
    let repo = directory();
    init(&repo);
    let out = run_program_without_terminal_with_cwd(Path::new(EXE), &[], &home, Some(&repo));
    assert_repository(&out);
}

#[test]
fn a_relative_directory_is_accepted() {
    if !git_available() {
        return;
    }
    let home = directory();
    let parent = directory();
    init(&parent.join("relative"));
    let out = run_program_without_terminal_with_cwd(
        Path::new(EXE),
        &[OsStr::new("relative")],
        &home,
        Some(&parent),
    );
    assert_repository(&out);
}

#[test]
fn a_directory_with_spaces_is_accepted() {
    if !git_available() {
        return;
    }
    let home = directory();
    let repo = home.join("repository with spaces");
    init(&repo);
    assert_repository(&run(&home, &[repo.as_os_str()]));
}

#[cfg(unix)]
#[test]
fn a_non_utf8_directory_is_accepted_as_bytes() {
    use std::os::unix::ffi::OsStrExt;

    if !git_available() {
        return;
    }
    let home = directory();
    let repo = home.join(OsStr::from_bytes(b"repository-\xff"));
    if let Err(error) = std::fs::create_dir(&repo) {
        // macOS filesystems can reject the fixture itself with EILSEQ. Keep the byte-path
        // assertion active on filesystems that can represent this name (including Linux).
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(92) {
            eprintln!("SKIP: this macOS filesystem rejects non-UTF-8 directory names (EILSEQ); run this case on Linux");
            return;
        }
        panic!("create non-UTF-8 repository fixture: {error}");
    }
    init(&repo);
    assert_repository(&run(&home, &[repo.as_os_str()]));
}

#[cfg(unix)]
#[test]
fn a_non_utf8_missing_path_is_reported_without_panicking() {
    use std::os::unix::ffi::OsStrExt;

    let home = directory();
    let missing = home.join(OsStr::from_bytes(b"missing-\xff"));
    let error = std::fs::metadata(&missing).unwrap_err();
    let (code, reason) = if error.kind() == std::io::ErrorKind::NotFound {
        (2, "no such directory")
    } else {
        // A filesystem that rejects the byte name reports a discovery I/O error instead.
        (1, "cannot detect the project")
    };
    let err = assert_reason(&run(&home, &[missing.as_os_str()]), code, reason);
    assert!(!err.contains(TERMINAL), "{err}");
}

#[test]
fn a_missing_directory_is_named_and_refused_before_the_terminal() {
    let home = directory();
    let missing = home.join("missing-repository");
    let err = assert_reason(&run(&home, &[missing.as_os_str()]), 2, "no such directory");
    assert!(err.contains("missing-repository"), "{err}");
    assert!(!err.contains(TERMINAL), "{err}");
}

#[test]
fn a_file_is_refused_before_the_terminal() {
    let home = directory();
    let file = home.join("file");
    std::fs::write(&file, "fixture").unwrap();
    let err = assert_reason(&run(&home, &[file.as_os_str()]), 2, "not a directory");
    assert!(!err.contains(TERMINAL), "{err}");
}

#[test]
fn two_directories_are_refused() {
    let home = directory();
    assert_reason(
        &run(&home, &[home.as_os_str(), home.as_os_str()]),
        2,
        "give one directory to open",
    );
}

#[test]
fn an_unknown_option_is_refused() {
    let home = directory();
    let err = assert_reason(&run(&home, &[OsStr::new("--unknown")]), 2, "unknown option");
    assert!(err.contains("--unknown"), "{err}");
}

#[test]
fn a_plain_directory_is_refused_before_the_terminal() {
    let home = directory();
    let err = assert_reason(
        &run(&home, &[home.as_os_str()]),
        2,
        "this program does not show a project here",
    );
    assert!(!err.contains(TERMINAL), "{err}");
}

#[test]
fn fake_git_markers_are_refused_by_the_backend_before_the_terminal() {
    if !git_available() {
        return;
    }
    let home = directory();
    for is_dir in [false, true] {
        let repo = directory();
        let marker = repo.join(".git");
        if is_dir {
            std::fs::create_dir(&marker).unwrap();
        } else {
            std::fs::write(&marker, "").unwrap();
        }
        let err = assert_reason(&run(&home, &[repo.as_os_str()]), 1, BACKEND);
        assert!(err.contains("cannot open the project"), "{err}");
        assert!(!err.contains(TERMINAL), "{err}");
    }
}

#[test]
fn an_unborn_repository_is_accepted() {
    if !git_available() {
        return;
    }
    let home = directory();
    let repo = directory();
    init(&repo);
    let head = Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .unwrap();
    assert!(!head.status.success(), "the fixture has no commit");
    assert_repository(&run(&home, &[repo.as_os_str()]));
}

#[test]
fn a_broken_config_warns_exactly_once_and_continues() {
    if !git_available() {
        return;
    }
    let home = directory();
    let repo = directory();
    init(&repo);
    let config_dir = home.join("xdg/konoma");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "[ui\n").unwrap();
    let out = run(&home, &[repo.as_os_str()]);
    assert_repository(&out);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        err.matches("config error (using defaults):").count(),
        1,
        "{err}"
    );
    assert!(
        err.contains("expected `]`"),
        "the warning includes the parse reason: {err}"
    );
}

#[cfg(feature = "git")]
#[test]
fn a_jj_workspace_is_accepted_when_selected() {
    if !konoma_vcs_core::vcs::jj::available() {
        eprintln!("SKIP: jj is not on PATH; Jujutsu program case was not run");
        return;
    }
    let home = directory();
    let repo = directory();
    let out = Command::new("jj")
        .current_dir(&repo)
        .env("HOME", home.as_path())
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("JJ_CONFIG", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["git", "init", "--no-colocate", "."])
        .output()
        .unwrap();
    assert!(out.status.success(), "jj init: {out:?}");
    assert_reason(
        &run(
            &home,
            &[OsStr::new("--plugin"), OsStr::new("jj"), repo.as_os_str()],
        ),
        1,
        TERMINAL,
    );
    assert_reason(
        &run(
            &home,
            &[OsStr::new("--plugin"), OsStr::new("git"), repo.as_os_str()],
        ),
        2,
        "this program does not show a project here",
    );
}

#[test]
fn manifests_name_only_the_two_project_plugins() {
    let home = directory();
    for id in ["git", "jj"] {
        let out = run(
            &home,
            &[
                OsStr::new("--manifest"),
                OsStr::new("--plugin"),
                OsStr::new(id),
            ],
        );
        assert!(out.stderr.is_empty(), "{out:?}");
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let json: String = String::from_utf8(out.stdout)
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(json.contains(&format!("\"id\":\"{id}\"")), "{json}");
    }
    let err = assert_reason(
        &run(&home, &[OsStr::new("--manifest")]),
        2,
        "several plugins (git, jj)",
    );
    assert!(err.contains("--plugin <id>"), "{err}");
}

#[test]
fn version_lists_git_then_jj_and_no_file_plugin() {
    let home = directory();
    let out = run(&home, &[OsStr::new("--version")]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let version = String::from_utf8(out.stdout).unwrap();
    let ids: Vec<_> = version
        .lines()
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(ids, ["git", "jj"], "{version}");
}

#[test]
fn preview_plugins_are_never_manifested_or_served() {
    let home = directory();
    for mode in ["--manifest", "--serve"] {
        let out = run(
            &home,
            &[
                OsStr::new(mode),
                OsStr::new("--plugin"),
                OsStr::new("markdown"),
            ],
        );
        let err = assert_reason(&out, 2, "has no plugin `markdown`");
        assert!(err.contains("it has: git, jj"), "{err}");
        assert!(out.stdout.is_empty(), "{out:?}");
    }
}

#[test]
fn help_succeeds_without_a_terminal() {
    let home = directory();
    let out = run(&home, &[OsStr::new("--help")]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains("USAGE:") && help.contains("--serve"),
        "{help}"
    );
}
